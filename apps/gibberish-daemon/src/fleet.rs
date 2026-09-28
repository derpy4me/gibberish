//! In-memory fleet tracking, telemetry line parsing, and live ANSI TUI dashboard (R9, R10).

use gibberish_protocol::{
    compare_epoch, CompactDeltaPayload, DiagnosticEventCode, EpochComparison,
    StaticMetadataBeacon, StorageModeStatus, TelemetryTier,
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerContextState {
    Active,
    PendingContext { first_seen_secs: u64 },
    AmbiguousEpoch { detected_secs: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTelemetry {
    pub node_id: String,
    pub tier: String,
    pub storage_mode: String,
    pub uptime_secs: u32,
    pub rx_count: u32,
    pub tx_count: u32,
    pub drop_count: u32,
    pub rssi: i8,
    pub lqi: u8,
    pub last_seen: Instant,
    pub state: PeerContextState,
    pub config_epoch: u8,
    pub uptime_epoch: u16,
    pub hw_rev: u8,
    pub schema_version: u8,
    pub sram_used: u16,
    pub free_heap_kb: u8,
    pub last_event: DiagnosticEventCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDongleStatus {
    pub node_id: u32,
    pub uptime_secs: u32,
    pub storage_mode: String,
    pub storage_stats: String,
    pub drop_count: u32,
}

/// Parses local hardware dongle diagnostic heartbeat line:
/// e.g. "[Node BEBCE5B8] Uptime: 67924s | Storage: MicroSdActive | SRAM: 0/256 pkts | Drops: 0"
pub fn parse_local_heartbeat_line(line: &str) -> Option<LocalDongleStatus> {
    let node_start = line.find("[Node ")?;
    let remainder = &line[node_start + "[Node ".len()..];
    let node_end = remainder.find(']')?;
    let node_hex = remainder[..node_end].trim();
    let node_id = u32::from_str_radix(node_hex, 16).ok()?;

    let mut uptime_secs = 0u32;
    let mut storage_mode = "RAM ONLY".to_string();
    let mut sram_str = "0/256 pkts".to_string();
    let mut drop_count = 0u32;

    for part in remainder[node_end + 1..].split('|') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            if key == "Uptime" {
                let num_str = val.trim_end_matches('s').trim();
                if let Ok(n) = num_str.parse::<u32>() {
                    uptime_secs = n;
                }
            } else if key == "Storage" {
                if val.contains("MicroSd") || val.contains("SD") || val.contains("Sd") {
                    storage_mode = "SD ACTIVE".to_string();
                } else {
                    storage_mode = "RAM ONLY".to_string();
                }
            } else if key == "SRAM" {
                sram_str = val.to_string();
            } else if key == "Drops" {
                if let Ok(n) = val.parse::<u32>() {
                    drop_count = n;
                }
            }
        }
    }

    let storage_stats = if storage_mode == "SD ACTIVE" {
        format!("MicroSD Active | SRAM: {}", sram_str)
    } else {
        format!("RAM Only | SRAM: {}", sram_str)
    };

    Some(LocalDongleStatus {
        node_id,
        uptime_secs,
        storage_mode,
        storage_stats,
        drop_count,
    })
}

/// Parses a structured telemetry line emitted over CDC or wireless 802.15.4.
/// Returns None if the line does not represent a valid telemetry frame.
pub fn parse_telemetry_line(line: &str) -> Option<NodeTelemetry> {
    // Locate telemetry marker: either "[Telemetry RX]" or "Telemetry from:"
    let content = if let Some(pos) = line.find("[Telemetry RX]") {
        &line[pos + "[Telemetry RX]".len()..]
    } else {
        let pos = line.find("Telemetry from:")?;
        &line[pos + "Telemetry from:".len()..]
    };

    let mut node_id = String::new();
    let mut tier = "DEBUG".to_string();
    let mut storage_mode = "SD ACTIVE".to_string();
    let mut uptime_secs = 0u32;
    let mut rx_count = 0u32;
    let mut tx_count = 0u32;
    let mut drop_count = 0u32;
    let mut rssi = -50i8;
    let mut lqi = 255u8;
    let mut config_epoch = 1u8;
    let mut uptime_epoch = 0u16;
    let mut sram_used = 0u16;
    let free_heap_kb = 0u8;
    let last_event = DiagnosticEventCode::RadioRxOk;

    // Parse comma-separated or key-value segments from content: Key: Value
    for part in content.split(',') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            let val = v.trim();

            if key.ends_with("Node") || key == "Node" {
                let id = val.split_whitespace().next().unwrap_or("").trim();
                let clean_id = id.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                if !clean_id.is_empty() && clean_id.len() >= 4 {
                    node_id = clean_id.to_uppercase();
                }
            } else if key == "Tier" {
                if val.eq_ignore_ascii_case("Prod") || val.eq_ignore_ascii_case("PROD") {
                    tier = "PROD".to_string();
                } else {
                    tier = "DEBUG".to_string();
                }
            } else if key == "Storage" {
                if val.contains("RAM") || val.contains("Ram") {
                    storage_mode = "RAM ONLY".to_string();
                } else {
                    storage_mode = "SD ACTIVE".to_string();
                }
            } else if key == "Uptime" {
                let num_str = val.trim_end_matches('s').trim();
                if let Ok(n) = num_str.parse::<u32>() {
                    uptime_secs = n;
                }
            } else if key == "RX" {
                if let Ok(n) = val.parse::<u32>() {
                    rx_count = n;
                }
            } else if key == "TX" {
                if let Ok(n) = val.parse::<u32>() {
                    tx_count = n;
                }
            } else if key == "Drops" {
                if let Ok(n) = val.parse::<u32>() {
                    drop_count = n;
                }
            } else if key == "RSSI" {
                let num_str = val.split_whitespace().next().unwrap_or("").trim();
                if let Ok(n) = num_str.parse::<i8>() {
                    rssi = n;
                }
            } else if key == "LQI" {
                let num_str = val.split_whitespace().next().unwrap_or("").trim();
                if let Ok(n) = num_str.parse::<u8>() {
                    lqi = n;
                }
            } else if key == "Epoch" {
                if let Ok(n) = val.parse::<u8>() {
                    config_epoch = n;
                }
            } else if key == "UptimeEpoch" {
                if let Ok(n) = val.parse::<u16>() {
                    uptime_epoch = n;
                }
            } else if key == "SRAM" {
                let num_str = val.split('/').next().unwrap_or("").trim();
                if let Ok(n) = num_str.parse::<u16>() {
                    sram_used = n;
                }
            }
        } else if node_id.is_empty() {
            // Alternative prefix format: e.g. "Telemetry from: BEBCE5B8"
            let clean = trimmed.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_ascii_alphanumeric());
            if clean.len() >= 4 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
                node_id = clean.to_uppercase();
            }
        }
    }

    if node_id.is_empty() {
        return None;
    }

    Some(NodeTelemetry {
        node_id,
        tier,
        storage_mode,
        uptime_secs,
        rx_count,
        tx_count,
        drop_count,
        rssi,
        lqi,
        last_seen: Instant::now(),
        state: PeerContextState::Active,
        config_epoch,
        uptime_epoch,
        hw_rev: 1,
        schema_version: 1,
        sram_used,
        free_heap_kb,
        last_event,
    })
}

pub struct FleetManager {
    nodes: HashMap<String, NodeTelemetry>,
    log_file: Option<File>,
    log_path: PathBuf,
}

impl FleetManager {
    pub fn new(log_path: impl Into<PathBuf>) -> Self {
        let path = log_path.into();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();

        Self {
            nodes: HashMap::new(),
            log_file: file,
            log_path: path,
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn get_node(&self, node_id: &str) -> Option<&NodeTelemetry> {
        self.nodes.get(node_id)
    }

    pub fn ingest_line(&mut self, line: &str) -> Option<NodeTelemetry> {
        if let Some(mut telemetry) = parse_telemetry_line(line) {
            if let Some(existing) = self.nodes.get_mut(&telemetry.node_id) {
                if telemetry.uptime_epoch == 0 && existing.uptime_epoch != 0 {
                    telemetry.uptime_epoch = existing.uptime_epoch;
                    telemetry.hw_rev = existing.hw_rev;
                    telemetry.schema_version = existing.schema_version;
                }
                if !line.contains("Epoch:") && existing.config_epoch != 0 {
                    telemetry.config_epoch = existing.config_epoch;
                }
                if existing.state != PeerContextState::Active && telemetry.state == PeerContextState::Active {
                    if line.contains("Epoch:") && line.contains("UptimeEpoch:") {
                        telemetry.state = PeerContextState::Active;
                    } else {
                        telemetry.state = existing.state;
                    }
                }
                *existing = telemetry.clone();
            } else {
                self.nodes.insert(telemetry.node_id.clone(), telemetry.clone());
            }
            self.log_entry(&telemetry);
            Some(telemetry)
        } else {
            None
        }
    }

    pub fn ingest_delta(
        &mut self,
        src_node_id: u32,
        delta: &CompactDeltaPayload,
        rssi: i8,
        lqi: u8,
    ) -> NodeTelemetry {
        let node_id_str = format!("{:08X}", src_node_id);
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(existing) = self.nodes.get_mut(&node_id_str) {
            existing.uptime_secs = delta.uptime_secs;
            existing.rx_count = delta.rx_count;
            existing.tx_count = delta.tx_count;
            existing.drop_count = delta.drop_count;
            existing.sram_used = delta.sram_used;
            existing.free_heap_kb = delta.free_heap_kb;
            existing.last_event = delta.last_event;
            existing.rssi = rssi;
            existing.lqi = lqi;
            existing.last_seen = Instant::now();

            if delta.config_epoch != existing.config_epoch {
                match compare_epoch(delta.config_epoch, existing.config_epoch) {
                    EpochComparison::Newer => {
                        existing.config_epoch = delta.config_epoch;
                        existing.state = PeerContextState::PendingContext {
                            first_seen_secs: now_secs,
                        };
                    }
                    EpochComparison::Ambiguous => {
                        existing.state = PeerContextState::AmbiguousEpoch {
                            detected_secs: now_secs,
                        };
                    }
                    _ => {}
                }
            }

            let ret = existing.clone();
            self.log_entry(&ret);
            ret
        } else {
            let telem = NodeTelemetry {
                node_id: node_id_str.clone(),
                tier: "PROD".to_string(),
                storage_mode: "SD ACTIVE".to_string(),
                uptime_secs: delta.uptime_secs,
                rx_count: delta.rx_count,
                tx_count: delta.tx_count,
                drop_count: delta.drop_count,
                rssi,
                lqi,
                last_seen: Instant::now(),
                state: PeerContextState::PendingContext {
                    first_seen_secs: now_secs,
                },
                config_epoch: delta.config_epoch,
                uptime_epoch: 0,
                hw_rev: 1,
                schema_version: 1,
                sram_used: delta.sram_used,
                free_heap_kb: delta.free_heap_kb,
                last_event: delta.last_event,
            };
            self.log_entry(&telem);
            self.nodes.insert(node_id_str, telem.clone());
            telem
        }
    }

    pub fn ingest_static_beacon(
        &mut self,
        beacon: &StaticMetadataBeacon,
        rssi: i8,
        lqi: u8,
    ) -> NodeTelemetry {
        let node_id_str = format!("{:08X}", beacon.node_id_u32());
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let tier_str = match beacon.build_tier {
            TelemetryTier::Prod => "PROD",
            TelemetryTier::Debug => "DEBUG",
        }.to_string();

        let storage_str = match beacon.storage_mode {
            StorageModeStatus::MicroSdActive => "SD ACTIVE",
            StorageModeStatus::RamOnly => "RAM ONLY",
        }.to_string();

        if let Some(existing) = self.nodes.get_mut(&node_id_str) {
            existing.tier = tier_str;
            existing.storage_mode = storage_str;
            existing.uptime_epoch = beacon.uptime_epoch;
            existing.hw_rev = beacon.hw_rev;
            existing.schema_version = beacon.schema_version;
            existing.rssi = rssi;
            existing.lqi = lqi;
            existing.last_seen = Instant::now();

            match compare_epoch(beacon.config_epoch, existing.config_epoch) {
                EpochComparison::Newer => {
                    existing.config_epoch = beacon.config_epoch;
                    existing.state = PeerContextState::Active;
                }
                EpochComparison::Equal => {
                    existing.state = PeerContextState::Active;
                }
                EpochComparison::Ambiguous => {
                    existing.state = PeerContextState::AmbiguousEpoch {
                        detected_secs: now_secs,
                    };
                }
                EpochComparison::Older => {}
            }

            let ret = existing.clone();
            self.log_entry(&ret);
            ret
        } else {
            let telem = NodeTelemetry {
                node_id: node_id_str.clone(),
                tier: tier_str,
                storage_mode: storage_str,
                uptime_secs: 0,
                rx_count: 0,
                tx_count: 0,
                drop_count: 0,
                rssi,
                lqi,
                last_seen: Instant::now(),
                state: PeerContextState::Active,
                config_epoch: beacon.config_epoch,
                uptime_epoch: beacon.uptime_epoch,
                hw_rev: beacon.hw_rev,
                schema_version: beacon.schema_version,
                sram_used: 0,
                free_heap_kb: 0,
                last_event: DiagnosticEventCode::Boot,
            };
            self.log_entry(&telem);
            self.nodes.insert(node_id_str, telem.clone());
            telem
        }
    }

    pub fn log_entry(&mut self, t: &NodeTelemetry) {
        if let Some(ref mut f) = self.log_file {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let _ = writeln!(
                f,
                "[{}] Node {} ({}) | Uptime: {}s | Storage: {} | RX: {} TX: {} Drops: {} | RSSI: {} dBm LQI: {}",
                now, t.node_id, t.tier, t.uptime_secs, t.storage_mode, t.rx_count, t.tx_count, t.drop_count, t.rssi, t.lqi
            );
            let _ = f.flush();
        }
    }

    pub fn format_uptime(secs: u32) -> String {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{:02}:{:02}:{:02}", h, m, s)
    }

    pub fn render_dashboard(&self, listening_ports: &[String]) -> String {
        let mut out = String::new();
        let border = "=".repeat(80);
        let divider = "-".repeat(80);
        let ports_str = if listening_ports.is_empty() {
            "None".to_string()
        } else {
            listening_ports.join(", ")
        };

        out.push_str(&format!("\x1B[1;36m{}\x1B[0m\n", border));
        out.push_str(" PROJECT GIBBERISH - 802.15.4 OFF-GRID FLEET SINK\n");
        out.push_str(&format!(" Listening on: {} | Logging to: {}\n", ports_str, self.log_path.display()));
        out.push_str(&format!("\x1B[1;36m{}\x1B[0m\n", border));
        out.push_str("\x1B[1mNODE ID   STATUS         TIER   STORAGE     UPTIME     RX    TX   DROPS  RSSI   LQI  LAST SEEN\x1B[0m\n");
        out.push_str(&format!("{}\n", divider));

        if self.nodes.is_empty() {
            out.push_str(" (No telemetry frames received yet - waiting for 802.15.4 beacons...)\n");
        } else {
            let mut sorted_nodes: Vec<&NodeTelemetry> = self.nodes.values().collect();
            sorted_nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));

            for node in sorted_nodes {
                let elapsed_secs = node.last_seen.elapsed().as_secs();
                let status_str = if elapsed_secs > 360 {
                    "\x1B[31m[STALE]\x1B[0m       "
                } else {
                    match node.state {
                        PeerContextState::Active => "\x1B[32m[Active]\x1B[0m      ",
                        PeerContextState::PendingContext { .. } => "\x1B[33m[Pending Sync]\x1B[0m",
                        PeerContextState::AmbiguousEpoch { .. } => "\x1B[31m[AMBIGUOUS]\x1B[0m   ",
                    }
                };

                let (color_code, last_seen_str) = if elapsed_secs <= 10 {
                    ("\x1B[32m", format!("{}s ago", elapsed_secs))
                } else if elapsed_secs <= 30 {
                    ("\x1B[33m", format!("{}s ago", elapsed_secs))
                } else {
                    ("\x1B[31m", format!("{}s ago (STALE)", elapsed_secs))
                };

                let row = format!(
                    "{:<8}  {}  {:<5}  {:<10}  {:<8} {:>5} {:>5} {:>7}  {:>4}   {:>3}  {}{:>13}\x1B[0m\n",
                    node.node_id,
                    status_str,
                    node.tier,
                    node.storage_mode,
                    Self::format_uptime(node.uptime_secs),
                    node.rx_count,
                    node.tx_count,
                    node.drop_count,
                    node.rssi,
                    node.lqi,
                    color_code,
                    last_seen_str
                );
                out.push_str(&row);
            }
        }
        out.push_str(&format!("\x1B[1;36m{}\x1B[0m\n", border));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_telemetry_line_valid_debug() {
        let line = "[Telemetry RX] Node: BEBCE5B8, Tier: Debug, Storage: MicroSdActive, Uptime: 862s, SRAM: 0/256, Drops: 0, RX: 904, TX: 68, RSSI: -19 dBm, LQI: 255";
        let telem = parse_telemetry_line(line).expect("Should parse valid debug telemetry line");
        assert_eq!(telem.node_id, "BEBCE5B8");
        assert_eq!(telem.tier, "DEBUG");
        assert_eq!(telem.storage_mode, "SD ACTIVE");
        assert_eq!(telem.uptime_secs, 862);
        assert_eq!(telem.drop_count, 0);
        assert_eq!(telem.rx_count, 904);
        assert_eq!(telem.tx_count, 68);
        assert_eq!(telem.rssi, -19);
        assert_eq!(telem.lqi, 255);
    }

    #[test]
    fn test_parse_telemetry_line_valid_prod_ram() {
        let line = "[Dongle /dev/ttyACM1] [Telemetry RX] Node: BEBD82B4, Tier: Prod, Storage: RamOnly, Uptime: 838s, SRAM: 46/256, Drops: 46, RX: 302, TX: 33, RSSI: -21 dBm, LQI: 240";
        let telem = parse_telemetry_line(line).expect("Should parse valid prod telemetry line");
        assert_eq!(telem.node_id, "BEBD82B4");
        assert_eq!(telem.tier, "PROD");
        assert_eq!(telem.storage_mode, "RAM ONLY");
        assert_eq!(telem.uptime_secs, 838);
        assert_eq!(telem.drop_count, 46);
        assert_eq!(telem.rx_count, 302);
        assert_eq!(telem.tx_count, 33);
        assert_eq!(telem.rssi, -21);
        assert_eq!(telem.lqi, 240);
    }

    #[test]
    fn test_parse_telemetry_line_with_timestamp_prefix() {
        let line = "2026-09-22 14:27:18 [Telemetry RX] Node: BEBCE5B8, Tier: Debug, Storage: MicroSdActive, Uptime: 862s, SRAM: 0/256, Drops: 0, RX: 904, TX: 68, RSSI: -19 dBm, LQI: 255";
        let telem = parse_telemetry_line(line).expect("Should parse line with timestamp prefix");
        assert_eq!(telem.node_id, "BEBCE5B8");
        assert_eq!(telem.tier, "DEBUG");
        assert_eq!(telem.uptime_secs, 862);
    }

    #[test]
    fn test_parse_telemetry_line_robustness_on_malformed() {
        assert!(parse_telemetry_line("[Radio RX] MsgID: 12345678, Chunk: 0/1").is_none());
        assert!(parse_telemetry_line("[Telemetry RX] Corrupted garbage without node").is_none());
        assert!(parse_telemetry_line("").is_none());
        assert!(parse_telemetry_line("[Telemetry RX] Node: ").is_none());
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(FleetManager::format_uptime(0), "00:00:00");
        assert_eq!(FleetManager::format_uptime(59), "00:00:59");
        assert_eq!(FleetManager::format_uptime(60), "00:01:00");
        assert_eq!(FleetManager::format_uptime(3661), "01:01:01");
        assert_eq!(FleetManager::format_uptime(862), "00:14:22");
    }

    #[test]
    fn test_fleet_manager_ingest_and_render() {
        let log_path = "/tmp/gibberish_test_fleet.log";
        let mut fleet = FleetManager::new(log_path);

        let line1 = "[Telemetry RX] Node: BEBCE5B8, Tier: Debug, Storage: MicroSdActive, Uptime: 862s, SRAM: 0/256, Drops: 0, RX: 904, TX: 68, RSSI: -19 dBm, LQI: 255";
        let line2 = "[Telemetry RX] Node: BEBD82B4, Tier: Prod, Storage: RamOnly, Uptime: 838s, SRAM: 46/256, Drops: 46, RX: 302, TX: 33, RSSI: -19 dBm, LQI: 255";

        assert!(fleet.ingest_line(line1).is_some());
        assert!(fleet.ingest_line(line2).is_some());
        assert_eq!(fleet.node_count(), 2);

        let dashboard = fleet.render_dashboard(&["/dev/ttyACM0".to_string(), "/dev/ttyACM1".to_string()]);
        assert!(dashboard.contains("BEBCE5B8"));
        assert!(dashboard.contains("BEBD82B4"));
        assert!(dashboard.contains("SD ACTIVE"));
        assert!(dashboard.contains("RAM ONLY"));
        assert!(dashboard.contains("00:14:22"));
        assert!(dashboard.contains("PROJECT GIBBERISH - 802.15.4 OFF-GRID FLEET SINK"));

        let _ = fs::remove_file(log_path);
    }

    #[test]
    fn test_parse_local_heartbeat_line() {
        let line_sd = "[Node BEBCE5B8] Uptime: 67924s | Storage: MicroSdActive | SRAM: 0/256 pkts | Drops: 0";
        let status = parse_local_heartbeat_line(line_sd).expect("should parse SD heartbeat");
        assert_eq!(status.node_id, 0xBEBCE5B8);
        assert_eq!(status.uptime_secs, 67924);
        assert_eq!(status.storage_mode, "SD ACTIVE");
        assert_eq!(status.storage_stats, "MicroSD Active | SRAM: 0/256 pkts");
        assert_eq!(status.drop_count, 0);

        let line_ram = "[Dongle /dev/cu.usbmodem1101] [Node BEBD82B4] Uptime: 64371s | Storage: RamOnly | SRAM: 7/256 pkts | Drops: 12";
        let status_ram = parse_local_heartbeat_line(line_ram).expect("should parse RAM heartbeat");
        assert_eq!(status_ram.node_id, 0xBEBD82B4);
        assert_eq!(status_ram.uptime_secs, 64371);
        assert_eq!(status_ram.storage_mode, "RAM ONLY");
        assert_eq!(status_ram.storage_stats, "RAM Only | SRAM: 7/256 pkts");
        assert_eq!(status_ram.drop_count, 12);
    }

    #[test]
    fn test_fleet_late_joiner_pending_context() {
        let log_path = "/tmp/gibberish_test_fleet_late.log";
        let mut fleet = FleetManager::new(log_path);

        let mut delta = CompactDeltaPayload::new();
        delta.uptime_secs = 42;
        delta.rx_count = 10;
        delta.tx_count = 5;
        delta.config_epoch = 3;

        let node_id = 0x11223344;
        let telem = fleet.ingest_delta(node_id, &delta, -25, 200);

        assert_eq!(telem.node_id, "11223344");
        assert_eq!(telem.uptime_secs, 42);
        assert_eq!(telem.config_epoch, 3);
        match telem.state {
            PeerContextState::PendingContext { first_seen_secs } => {
                assert!(first_seen_secs > 0);
            }
            _ => panic!("Expected PendingContext state for late joiner delta without beacon"),
        }

        let dashboard = fleet.render_dashboard(&[]);
        assert!(dashboard.contains("11223344"));
        assert!(dashboard.contains("Pending Sync"));

        let _ = fs::remove_file(log_path);
    }

    #[test]
    fn test_fleet_beacon_promotes_to_active() {
        let log_path = "/tmp/gibberish_test_fleet_promote.log";
        let mut fleet = FleetManager::new(log_path);

        let node_id = 0xAABBCCDD;
        let mut delta = CompactDeltaPayload::new();
        delta.uptime_secs = 100;
        delta.config_epoch = 1;
        fleet.ingest_delta(node_id, &delta, -30, 180);

        // Verify initially PendingContext
        let node = fleet.get_node("AABBCCDD").expect("Node should exist");
        assert!(matches!(node.state, PeerContextState::PendingContext { .. }));

        // Now receive StaticMetadataBeacon with same epoch (or newer)
        let mut beacon = StaticMetadataBeacon::new();
        beacon.node_id = [0, 0, 0, 0, 0xAA, 0xBB, 0xCC, 0xDD];
        beacon.config_epoch = 1;
        beacon.build_tier = TelemetryTier::Prod;
        beacon.storage_mode = StorageModeStatus::MicroSdActive;
        beacon.uptime_epoch = 0;

        let telem = fleet.ingest_static_beacon(&beacon, -28, 190);
        assert_eq!(telem.node_id, "AABBCCDD");
        assert_eq!(telem.tier, "PROD");
        assert_eq!(telem.storage_mode, "SD ACTIVE");
        assert_eq!(telem.state, PeerContextState::Active);

        let dashboard = fleet.render_dashboard(&[]);
        assert!(dashboard.contains("AABBCCDD"));
        assert!(dashboard.contains("Active"));

        let _ = fs::remove_file(log_path);
    }

    #[test]
    fn test_fleet_ambiguous_epoch_handling() {
        let log_path = "/tmp/gibberish_test_fleet_ambiguous.log";
        let mut fleet = FleetManager::new(log_path);

        let mut beacon = StaticMetadataBeacon::new();
        beacon.node_id = [0, 0, 0, 0, 0x12, 0x34, 0x56, 0x78];
        beacon.config_epoch = 10;
        fleet.ingest_static_beacon(&beacon, -20, 255);

        let node = fleet.get_node("12345678").unwrap();
        assert_eq!(node.state, PeerContextState::Active);
        assert_eq!(node.config_epoch, 10);

        // Ingest delta with diff == 128 (10 + 128 = 138)
        let mut delta = CompactDeltaPayload::new();
        delta.config_epoch = 138;
        delta.uptime_secs = 200;
        let telem = fleet.ingest_delta(0x12345678, &delta, -22, 240);

        match telem.state {
            PeerContextState::AmbiguousEpoch { detected_secs } => {
                assert!(detected_secs > 0);
            }
            _ => panic!("Expected AmbiguousEpoch state for diff == 128"),
        }

        let dashboard = fleet.render_dashboard(&[]);
        assert!(dashboard.contains("12345678"));
        assert!(dashboard.contains("AMBIGUOUS"));

        let _ = fs::remove_file(log_path);
    }
}

