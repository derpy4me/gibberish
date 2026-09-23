//! In-memory fleet tracking, telemetry line parsing, and live ANSI TUI dashboard (R9, R10).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
}

/// Parses a structured telemetry line emitted over CDC or wireless 802.15.4.
/// Returns None if the line does not represent a valid telemetry frame.
pub fn parse_telemetry_line(line: &str) -> Option<NodeTelemetry> {
    // Locate telemetry marker: either "[Telemetry RX]" or "Telemetry from:"
    let content = if let Some(pos) = line.find("[Telemetry RX]") {
        &line[pos + "[Telemetry RX]".len()..]
    } else if let Some(pos) = line.find("Telemetry from:") {
        &line[pos + "Telemetry from:".len()..]
    } else {
        return None;
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
        if let Some(telemetry) = parse_telemetry_line(line) {
            self.log_entry(&telemetry);
            self.nodes.insert(telemetry.node_id.clone(), telemetry.clone());
            Some(telemetry)
        } else {
            None
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
        out.push_str("\x1B[1mNODE ID   TIER   STORAGE     UPTIME     RX    TX   DROPS  RSSI   LQI  LAST SEEN\x1B[0m\n");
        out.push_str(&format!("{}\n", divider));

        if self.nodes.is_empty() {
            out.push_str(" (No telemetry frames received yet - waiting for 802.15.4 beacons...)\n");
        } else {
            let mut sorted_nodes: Vec<&NodeTelemetry> = self.nodes.values().collect();
            sorted_nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));

            for node in sorted_nodes {
                let elapsed_secs = node.last_seen.elapsed().as_secs();
                let (color_code, last_seen_str) = if elapsed_secs <= 10 {
                    ("\x1B[32m", format!("{}s ago", elapsed_secs))
                } else if elapsed_secs <= 30 {
                    ("\x1B[33m", format!("{}s ago", elapsed_secs))
                } else {
                    ("\x1B[31m", format!("{}s ago (STALE)", elapsed_secs))
                };

                let row = format!(
                    "{:<8}  {:<5}  {:<10}  {:<8} {:>5} {:>5} {:>7}  {:>4}   {:>3}  {}{:>13}\x1B[0m\n",
                    node.node_id,
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
}
