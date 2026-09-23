//! Project Gibberish Central Companion Fleet Sink & Live TUI Dashboard (`gibberish-sink`) (R9, R10).
//!
//! Listens to in-band IEEE 802.15.4 telemetry frames captured by local C5 dongles,
//! decodes peer metrics, appends entries to `/tmp/gibberish/fleet.log`, and renders
//! a live terminal fleet health matrix.

use gibberish_daemon::fleet::FleetManager;
use gibberish_daemon::transport::SerialTransport;
use std::io::{self, Write};
use std::time::{Duration, Instant};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut specified_ports: Vec<String> = Vec::new();
    let mut log_path = "/tmp/gibberish/fleet.log".to_string();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--port" || arg == "-p" {
            if let Some(p) = args.next() {
                for port in p.split(',') {
                    let trimmed = port.trim();
                    if !trimmed.is_empty() {
                        specified_ports.push(trimmed.to_string());
                    }
                }
            }
        } else if arg == "--log-file" || arg == "-l" {
            if let Some(l) = args.next() {
                log_path = l;
            }
        } else if arg == "--help" || arg == "-h" {
            println!("Project Gibberish - 802.15.4 Off-Grid Fleet Sink");
            println!("Usage: gibberish-sink [OPTIONS]");
            println!("\nOptions:");
            println!("  -p, --port <PORTS>      Comma-separated serial ports (default: auto-detect /dev/ttyACM*)");
            println!("  -l, --log-file <PATH>   Log file destination (default: /tmp/gibberish/fleet.log)");
            println!("  -h, --help              Print help");
            return Ok(());
        }
    }

    // Auto-discover /dev/ttyACM* ports if none specified
    if specified_ports.is_empty() {
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if name.starts_with("ttyACM") {
                        specified_ports.push(format!("/dev/{}", name));
                    }
                }
            }
        }
        specified_ports.sort();
    }

    println!("============================================================");
    println!(" Project Gibberish - Central 802.15.4 Fleet Sink Starting   ");
    println!(" Target ports: {:?}", specified_ports);
    println!(" Log destination: {}", log_path);
    println!("============================================================\n");

    let mut fleet = FleetManager::new(&log_path);
    let mut transports: Vec<(String, SerialTransport)> = Vec::new();

    for path in &specified_ports {
        match SerialTransport::open(path) {
            Ok(t) => {
                println!("✓ Connected to C5 dongle at {}", path);
                transports.push((path.clone(), t));
            }
            Err(e) => {
                eprintln!("Notice: Could not open {}: {}", path, e);
            }
        }
    }

    if transports.is_empty() {
        println!("⚠️ No active serial dongles opened. Will continue monitoring and rendering dashboard...");
    }

    let active_port_names: Vec<String> = if transports.is_empty() {
        specified_ports.clone()
    } else {
        transports.iter().map(|(p, _)| p.clone()).collect()
    };

    let mut last_render = Instant::now();
    let mut last_reconnect = Instant::now();

    loop {
        sleep(Duration::from_millis(50)).await;

        // Attempt reconnection if no transports are currently active (hotplug support)
        if transports.is_empty() && last_reconnect.elapsed() >= Duration::from_secs(3) {
            last_reconnect = Instant::now();
            let mut detected_ports = Vec::new();
            if let Ok(entries) = std::fs::read_dir("/dev") {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.starts_with("ttyACM") {
                            detected_ports.push(format!("/dev/{}", name));
                        }
                    }
                }
            }
            for path in detected_ports {
                if let Ok(t) = SerialTransport::open(&path) {
                    transports.push((path, t));
                }
            }
        }

        // Poll incoming lines from all open serial transports
        for (_port, t) in &mut transports {
            let lines = t.poll_dongle_lines();
            for line in lines {
                fleet.ingest_line(&line);
            }
        }

        // Render live dashboard every 1000ms
        if last_render.elapsed() >= Duration::from_millis(1000) {
            last_render = Instant::now();
            let ports: Vec<String> = transports.iter().map(|(p, _)| p.clone()).collect();
            let dashboard = fleet.render_dashboard(if ports.is_empty() { &active_port_names } else { &ports });

            // Clear screen and move cursor to top-left (ANSI)
            print!("\x1B[2J\x1B[H{}", dashboard);
            let _ = io::stdout().flush();
        }
    }
}
