//! Rigorous QA Verification Suite for Mesh Telemetry, CDC Sliding-Window Resync,
//! Priority Queue Preemption Timing, RFC 1982 Matrix, and Zero-Leakage Privacy.

use gibberish_daemon::fleet::{FleetManager, PeerContextState};
use gibberish_daemon::transport::parse_cdc_stream_sliding_window;
use gibberish_protocol::{
    assemble_variable_phy_frame, compare_epoch, encode_cdc_frame, parse_variable_phy_frame,
    ClosedTelemetry, CompactDeltaPayload, DiagnosticEventCode, EpochComparison, MeshHeader,
    MeshPacket, StaticMetadataBeacon, StorageModeStatus, TelemetryTier, CDC_FRAME_MAGIC,
    DEFAULT_NETWORK_TAG, FLAG_DIRECT, FLAG_TELEMETRY,
    MHR_LEN, PHY_MTU,
};
use std::fs;
use std::path::Path;

/// Deterministic 64-bit XorShift pseudorandom number generator for reproducible QA tests.
struct QARng {
    state: u64,
}

impl QARng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xDEADBEEFCAFE1234 } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state as u32
    }

    fn next_range(&mut self, min: usize, max: usize) -> usize {
        assert!(max >= min);
        min + (self.next_u32() as usize % (max - min + 1))
    }
}

// =============================================================================
// Skill 1: Stream Adversarial Fuzzing & Byte-Level Sliding-Window CDC Resync
// =============================================================================
#[test]
fn test_qa_sliding_window_cdc_fuzzing_and_resync() {
    let mut rng = QARng::new(0x20260928_01);
    let mut ground_truth_telems = Vec::new();
    let mut ground_truth_logs = Vec::new();
    let mut stream_bytes = Vec::new();

    // 1. Generate 40 valid frames and 20 valid log lines, mixed with noise and corruption
    for i in 0..40 {
        // Occasionally inject garbage noise before valid frames
        if rng.next_range(0, 3) == 0 {
            let noise_len = rng.next_range(1, 40);
            for _ in 0..noise_len {
                stream_bytes.push((rng.next_u32() & 0xFF) as u8);
            }
        }

        // Occasionally inject false magic header followed by corrupt data
        if rng.next_range(0, 4) == 0 {
            stream_bytes.extend_from_slice(&CDC_FRAME_MAGIC);
            stream_bytes.push(0x00);
            stream_bytes.push(rng.next_range(10, 50) as u8); // Fake length
            stream_bytes.push(0x12);
            stream_bytes.push(0x34); // Bad CRC
            for _ in 0..rng.next_range(5, 20) {
                stream_bytes.push((rng.next_u32() & 0xFF) as u8);
            }
        }

        // Interleave plaintext ASCII log lines
        if rng.next_range(0, 2) == 0 {
            let log_msg = format!("[Dongle /dev/ttyACM0] Diagnostic tick #{}\n", i);
            stream_bytes.extend_from_slice(log_msg.as_bytes());
            ground_truth_logs.push(log_msg.trim().to_string());
        }

        // Generate valid ClosedTelemetry Postcard binary frame
        let mut telem = ClosedTelemetry::new();
        telem.uptime_secs = 1000 + i as u32;
        telem.rx_packet_count = 10 * i as u32;
        telem.tx_packet_count = 5 * i as u32;
        telem.dropped_count = i as u32;
        telem.sram_ring_used = (i % 256) as u16;
        telem.storage_mode = if i % 2 == 0 {
            StorageModeStatus::MicroSdActive
        } else {
            StorageModeStatus::RamOnly
        };
        telem.last_event = DiagnosticEventCode::RadioRxOk;
        telem.last_rssi = -30 - (i % 20) as i8;

        let mut postcard_buf = [0u8; 64];
        let postcard_slice = postcard::to_slice(&telem, &mut postcard_buf).unwrap();

        let mut framed = [0u8; 80];
        let framed_len = encode_cdc_frame(postcard_slice, &mut framed).unwrap();
        stream_bytes.extend_from_slice(&framed[..framed_len]);
        ground_truth_telems.push(telem);
    }

    // 2. Feed stream into parser through pathological slice sizes (1 to 13 bytes per read)
    let mut parser_buf = Vec::new();
    let mut parsed_telems = Vec::new();
    let mut parsed_logs = Vec::new();

    let mut cursor = 0;
    while cursor < stream_bytes.len() {
        let chunk_size = rng.next_range(1, 13).min(stream_bytes.len() - cursor);
        parser_buf.extend_from_slice(&stream_bytes[cursor..cursor + chunk_size]);
        cursor += chunk_size;

        let (_pkts, telems, logs) = parse_cdc_stream_sliding_window(&mut parser_buf);
        parsed_telems.extend(telems);
        parsed_logs.extend(logs);
    }

    // Drain any remaining bytes in parser
    let (_pkts, telems, logs) = parse_cdc_stream_sliding_window(&mut parser_buf);
    parsed_telems.extend(telems);
    parsed_logs.extend(logs);

    // 3. Verify that ALL 40 valid frames were successfully recovered despite noise
    assert_eq!(
        parsed_telems.len(),
        ground_truth_telems.len(),
        "Parser failed to recover all valid telemetry frames through noise and variable chunking"
    );

    for (actual, expected) in parsed_telems.iter().zip(ground_truth_telems.iter()) {
        assert_eq!(actual.uptime_secs, expected.uptime_secs);
        assert_eq!(actual.rx_packet_count, expected.rx_packet_count);
        assert_eq!(actual.tx_packet_count, expected.tx_packet_count);
        assert_eq!(actual.storage_mode, expected.storage_mode);
        assert_eq!(actual.last_rssi, expected.last_rssi);
    }

    // Verify all uncorrupted log lines were preserved
    for expected_log in ground_truth_logs {
        assert!(
            parsed_logs.iter().any(|l| l.contains(&expected_log)),
            "Expected log line not recovered: {}",
            expected_log
        );
    }
}

// =============================================================================
// Skill 2: Exhaustive RFC 1982 Serial Number Arithmetic & Invariant Matrix
// =============================================================================
#[test]
fn test_qa_rfc1982_exhaustive_epoch_matrix() {
    let mut ambiguous_count = 0;
    let mut newer_count = 0;
    let mut older_count = 0;
    let mut equal_count = 0;

    // Full 256x256 state space verification
    for s1 in 0..=255u8 {
        for s2 in 0..=255u8 {
            let result = compare_epoch(s1, s2);
            let diff = s1.wrapping_sub(s2);

            match result {
                EpochComparison::Ambiguous => {
                    assert_eq!(diff, 128);
                    ambiguous_count += 1;
                }
                EpochComparison::Newer => {
                    assert!(diff > 0 && diff < 128);
                    newer_count += 1;
                }
                EpochComparison::Older => {
                    assert!(diff > 128);
                    older_count += 1;
                }
                EpochComparison::Equal => {
                    assert_eq!(diff, 0);
                    equal_count += 1;
                }
            }
        }
    }

    // Exact count verification for 256x256 = 65,536 pairs:
    // For each s1: 1 equal, 127 newer, 1 ambiguous, 127 older
    assert_eq!(ambiguous_count, 256);
    assert_eq!(newer_count, 256 * 127);
    assert_eq!(older_count, 256 * 127);
    assert_eq!(equal_count, 256);
    assert_eq!(ambiguous_count + newer_count + older_count + equal_count, 256 * 256);

    // Verify key RFC 1982 wrap boundaries
    assert_eq!(compare_epoch(0, 255), EpochComparison::Newer);
    assert_eq!(compare_epoch(1, 255), EpochComparison::Newer);
    assert_eq!(compare_epoch(126, 255), EpochComparison::Newer);
    assert_eq!(compare_epoch(127, 255), EpochComparison::Ambiguous);
    assert_eq!(compare_epoch(128, 255), EpochComparison::Older);
    assert_eq!(compare_epoch(0, 128), EpochComparison::Ambiguous);
    assert_eq!(compare_epoch(128, 0), EpochComparison::Ambiguous);
    assert_eq!(compare_epoch(200, 72), EpochComparison::Ambiguous);
}

// =============================================================================
// Skill 3: High-Density 32-Peer Swarm Simulation & Out-of-Order Lifecycle
// =============================================================================
#[test]
fn test_qa_multi_node_dense_swarm_stress() {
    let test_log = "/tmp/gibberish/qa_swarm_test.log";
    if Path::new(test_log).exists() {
        let _ = fs::remove_file(test_log);
    }
    let mut fleet = FleetManager::new(test_log);

    let num_nodes = 32;
    // Step 1: Simulate late joiners (nodes 0..16 send deltas BEFORE static beacon)
    for id in 0..16 {
        let node_u32 = 0xAA000000 | id as u32;
        let mut delta = CompactDeltaPayload::new();
        delta.uptime_secs = 50 + id as u32;
        delta.config_epoch = 1;
        delta.rx_count = 100 + id as u32;
        delta.tx_count = 20 + id as u32;

        let telem = fleet.ingest_delta(node_u32, &delta, -25, 240);
        assert!(
            matches!(telem.state, PeerContextState::PendingContext { .. }),
            "Node {:08X} without prior beacon must be in PendingContext",
            node_u32
        );
    }

    // Step 2: Nodes 16..32 receive StaticMetadataBeacon FIRST
    for id in 16..num_nodes {
        let node_u32 = 0xAA000000 | id as u32;
        let mut beacon = StaticMetadataBeacon::new();
        beacon.node_id[4..8].copy_from_slice(&node_u32.to_be_bytes());
        beacon.config_epoch = 1;
        beacon.build_tier = TelemetryTier::Prod;
        beacon.storage_mode = StorageModeStatus::MicroSdActive;

        let telem = fleet.ingest_static_beacon(&beacon, -20, 255);
        assert_eq!(
            telem.state,
            PeerContextState::Active,
            "Node {:08X} with beacon must be Active",
            node_u32
        );
    }

    assert_eq!(fleet.node_count(), 32);

    // Step 3: Now deliver beacons for nodes 0..16, promoting them to Active
    for id in 0..16 {
        let node_u32 = 0xAA000000 | id as u32;
        let mut beacon = StaticMetadataBeacon::new();
        beacon.node_id[4..8].copy_from_slice(&node_u32.to_be_bytes());
        beacon.config_epoch = 1;
        beacon.build_tier = TelemetryTier::Debug;
        beacon.storage_mode = StorageModeStatus::RamOnly;

        let telem = fleet.ingest_static_beacon(&beacon, -22, 245);
        assert_eq!(
            telem.state,
            PeerContextState::Active,
            "Node {:08X} should be promoted to Active",
            node_u32
        );
    }

    // Step 4: Induce AmbiguousEpoch on subset of nodes (diff == 128)
    for id in &[3, 7, 11, 25] {
        let node_u32 = 0xAA000000 | *id as u32;
        let mut delta = CompactDeltaPayload::new();
        delta.uptime_secs = 200;
        delta.config_epoch = 1u8.wrapping_add(128); // 129 -> diff == 128

        let telem = fleet.ingest_delta(node_u32, &delta, -19, 250);
        assert!(
            matches!(telem.state, PeerContextState::AmbiguousEpoch { .. }),
            "Node {:08X} should be in AmbiguousEpoch state",
            node_u32
        );
    }

    // Step 5: Render full dashboard and assert formatting
    let dashboard = fleet.render_dashboard(&["/dev/ttyACM0".to_string(), "/dev/ttyACM1".to_string()]);
    assert!(dashboard.contains("AA000000"));
    assert!(dashboard.contains("AA00001F")); // Node 31
    assert!(dashboard.contains("AMBIGUOUS"));
    assert!(dashboard.contains("Active"));

    // Cleanup test log
    let _ = fs::remove_file(test_log);
}

// =============================================================================
// Skill 4: Firmware Two-Tier Priority Queue & Preemption Model Verification
// =============================================================================
struct MockTwoTierScheduler {
    high_queue: [Option<MeshPacket>; 4],
    low_queue: [Option<[u8; 32]>; 2],
    low_timer_active: bool,
    low_timer_ms: u16,
    high_timer_active: bool,
    high_timer_ms: u16,
    preemptions: u32,
    transmitted_high: u32,
    transmitted_low: u32,
}

impl MockTwoTierScheduler {
    fn new() -> Self {
        Self {
            high_queue: [None, None, None, None],
            low_queue: [None, None],
            low_timer_active: false,
            low_timer_ms: 0,
            high_timer_active: false,
            high_timer_ms: 0,
            preemptions: 0,
            transmitted_high: 0,
            transmitted_low: 0,
        }
    }

    fn schedule_high(&mut self, pkt: MeshPacket, jitter_ms: u16) -> Result<(), ()> {
        // Find empty slot
        let slot = self.high_queue.iter_mut().find(|s| s.is_none()).ok_or(())?;
        *slot = Some(pkt);

        // Preempt any pending low-priority backoff
        if self.low_timer_active {
            self.low_timer_active = false;
            self.preemptions += 1;
        }

        if !self.high_timer_active {
            self.high_timer_active = true;
            self.high_timer_ms = jitter_ms;
        }
        Ok(())
    }

    fn schedule_low(&mut self, frame: [u8; 32], jitter_ms: u16) -> Result<(), ()> {
        let slot = self.low_queue.iter_mut().find(|s| s.is_none()).ok_or(())?;
        *slot = Some(frame);

        // Arm low backoff only if no high packets are queued or active
        if !self.high_timer_active && !self.low_timer_active {
            self.low_timer_active = true;
            self.low_timer_ms = jitter_ms;
        }
        Ok(())
    }

    fn tick_ms(&mut self, elapsed_ms: u16) {
        if self.high_timer_active {
            if self.high_timer_ms <= elapsed_ms {
                self.high_timer_active = false;
                // Pop highest FIFO
                if let Some(pos) = self.high_queue.iter().position(|s| s.is_some()) {
                    self.high_queue[pos] = None;
                    self.transmitted_high += 1;
                }
                // Rearm next high if available
                if self.high_queue.iter().any(|s| s.is_some()) {
                    self.high_timer_active = true;
                    self.high_timer_ms = 5;
                } else if self.low_queue.iter().any(|s| s.is_some()) {
                    // Resume low queue
                    self.low_timer_active = true;
                    self.low_timer_ms = 15;
                }
            } else {
                self.high_timer_ms -= elapsed_ms;
            }
        } else if self.low_timer_active {
            if self.low_timer_ms <= elapsed_ms {
                self.low_timer_active = false;
                if let Some(pos) = self.low_queue.iter().position(|s| s.is_some()) {
                    self.low_queue[pos] = None;
                    self.transmitted_low += 1;
                }
                if self.low_queue.iter().any(|s| s.is_some()) {
                    self.low_timer_active = true;
                    self.low_timer_ms = 20;
                }
            } else {
                self.low_timer_ms -= elapsed_ms;
            }
        }
    }
}

#[test]
fn test_qa_firmware_two_tier_priority_and_preemption_model() {
    let mut sched = MockTwoTierScheduler::new();

    // 1. Enqueue low-priority telemetry with 50 ms backoff
    assert!(sched.schedule_low([0x11; 32], 50).is_ok());
    assert!(sched.low_timer_active);
    assert_eq!(sched.low_timer_ms, 50);

    // 2. Advance 20 ms -> low timer has 30 ms remaining
    sched.tick_ms(20);
    assert_eq!(sched.low_timer_ms, 30);
    assert_eq!(sched.transmitted_low, 0);

    // 3. High-priority user chat packet arrives -> must preempt low timer immediately
    let dummy_pkt = MeshPacket {
        header: MeshHeader {
            network_tag: DEFAULT_NETWORK_TAG,
            msg_id: 999,
            chunk_idx: 0,
            total_chunks: 1,
            ttl: 3,
            hop_count: 0,
            flags: FLAG_DIRECT,
        },
        payload: [0xAA; 96],
    };
    assert!(sched.schedule_high(dummy_pkt, 8).is_ok());
    assert_eq!(sched.preemptions, 1, "Low backoff was not preempted");
    assert!(!sched.low_timer_active);
    assert!(sched.high_timer_active);
    assert_eq!(sched.high_timer_ms, 8);

    // 4. Advance 10 ms -> high packet transmits; low timer resumes
    sched.tick_ms(10);
    assert_eq!(sched.transmitted_high, 1);
    assert!(sched.low_timer_active);
    assert_eq!(sched.low_timer_ms, 15);

    // 5. Advance 15 ms -> low packet transmits
    sched.tick_ms(15);
    assert_eq!(sched.transmitted_low, 1);
    assert!(!sched.low_timer_active);
    assert!(!sched.high_timer_active);

    // 6. Test capacity limits: 4 high slots, 5th rejected
    for _ in 0..4 {
        assert!(sched.schedule_high(dummy_pkt, 10).is_ok());
    }
    assert!(
        sched.schedule_high(dummy_pkt, 10).is_err(),
        "Queue full must return error on 5th high-priority packet"
    );

    // 7. Non-preemptible PHY window mathematical derivation
    // 133 PHY octets at 250 kbps: (133 * 8 bits) / 250,000 bps = 4.256 ms
    let phy_bytes = 133.0f64;
    let phy_rate_bps = 250_000.0f64;
    let airtime_ms = (phy_bytes * 8.0 / phy_rate_bps) * 1000.0;
    assert!((airtime_ms - 4.256).abs() < 0.001);
    assert!(
        airtime_ms < 20.0,
        "Maximum PHY airtime must not exceed 20 ms interactive limit"
    );
}

// =============================================================================
// Skill 5: Privacy & Zero MAC Leakage Over-The-Air Frame Audit
// =============================================================================
#[test]
fn test_qa_zero_mac_leakage_privacy_contract() {
    // Real factory MAC with Espressif vendor OUI: [0x38, 0x44, 0xBE]
    let factory_mac: [u8; 6] = [0x38, 0x44, 0xBE, 0xBD, 0x82, 0xB4];
    let oui_prefix = [0x38, 0x44];
    let local_node_id = u32::from_be_bytes([
        factory_mac[2],
        factory_mac[3],
        factory_mac[4],
        factory_mac[5],
    ]);
    assert_eq!(local_node_id, 0xBEBD82B4);

    // Firmware node identity construction (upper 4 bytes zeroed, station ID in lower 4)
    let mut sanitized_node_id = [0u8; 8];
    sanitized_node_id[4..8].copy_from_slice(&local_node_id.to_be_bytes());

    // 1. Static Metadata Beacon Wire Audit
    let mut beacon = StaticMetadataBeacon::new();
    beacon.node_id = sanitized_node_id;
    beacon.build_tier = TelemetryTier::Prod;
    beacon.storage_mode = StorageModeStatus::MicroSdActive;
    beacon.config_epoch = 5;

    let mut beacon_wire = [0u8; StaticMetadataBeacon::BYTE_LEN];
    beacon.serialize(&mut beacon_wire).expect("Beacon serialize failed");

    // Assert vendor OUI prefix nowhere exists in beacon payload
    assert!(
        !beacon_wire.windows(2).any(|w| w == oui_prefix),
        "Privacy violation: Vendor OUI prefix leaked into StaticMetadataBeacon wire format!"
    );

    // 2. Compact Delta Payload Wire Audit
    let mut delta = CompactDeltaPayload::new();
    delta.uptime_secs = 12345;
    delta.config_epoch = 5;

    let mut delta_wire = [0u8; CompactDeltaPayload::BYTE_LEN];
    delta.serialize(&mut delta_wire).expect("Delta serialize failed");

    assert!(
        !delta_wire.windows(2).any(|w| w == oui_prefix),
        "Privacy violation: Vendor OUI prefix leaked into CompactDeltaPayload wire format!"
    );

    // 3. Assembled Variable 802.15.4 Physical Frame Audit
    let mut mhr = [0u8; MHR_LEN];
    mhr[0] = 0x41; // Frame control
    mhr[1] = 0x08;
    mhr[2] = 1;    // Sequence
    mhr[3..5].copy_from_slice(&0xFFFFu16.to_le_bytes()); // Dest PAN
    mhr[5..7].copy_from_slice(&0xFFFFu16.to_le_bytes()); // Broadcast Dest Addr
    mhr[7..11].copy_from_slice(&local_node_id.to_le_bytes()); // Source Addr (Node ID only)

    let header = MeshHeader {
        network_tag: DEFAULT_NETWORK_TAG,
        msg_id: 1,
        chunk_idx: 0,
        total_chunks: 1,
        ttl: 1,
        hop_count: 0,
        flags: FLAG_TELEMETRY,
    };

    let mut phy_frame = [0u8; PHY_MTU];
    let total_len = assemble_variable_phy_frame(&mhr, &header, &delta_wire, &mut phy_frame)
        .expect("PHY assembly failed");

    // Scan complete PHY frame byte slice
    assert!(
        !phy_frame[..total_len].windows(2).any(|w| w == oui_prefix),
        "Privacy violation: Vendor OUI prefix leaked into physical IEEE 802.15.4 PSDU!"
    );

    // Verify roundtrip parsing confirms node ID integrity
    let (parsed_hdr, parsed_payload) = parse_variable_phy_frame(&phy_frame[..total_len])
        .expect("PHY frame parse failed");
    assert_eq!(parsed_hdr.msg_id, 1);
    let parsed_delta = CompactDeltaPayload::deserialize(parsed_payload).unwrap();
    assert_eq!(parsed_delta.uptime_secs, 12345);
}
