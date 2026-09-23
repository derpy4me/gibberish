//! USB CDC-ACM Serial transport to LilyGO T-Dongle-C5 (R1, R3, R4, KTD2).

use gibberish_protocol::MeshPacket;
use serialport::SerialPort;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DONGLE_TO_HOST_LINE_LIMIT: usize = 1024;
pub const HOST_TO_DONGLE_FRAME_HEADER: [u8; 3] = [0xAA, 0x55, 0x72]; // 114 dec
pub const HOST_TO_DONGLE_TOTAL_LEN: usize = 3 + MeshPacket::WIRE_PAYLOAD_LEN; // 117

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedWirePacket {
    pub src_node_id: u32,
    pub packet: MeshPacket,
}

pub struct SerialTransport {
    port: Box<dyn SerialPort>,
    read_buf: Vec<u8>,
}

impl SerialTransport {
    pub fn open(port_path: &str) -> io::Result<Self> {
        let port = serialport::new(port_path, 115_200)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        Ok(Self {
            port,
            read_buf: Vec::with_capacity(DONGLE_TO_HOST_LINE_LIMIT),
        })
    }

    /// Auto-detect LilyGO T-Dongle-C5 serial device path.
    pub fn find_dongle() -> Option<PathBuf> {
        // Try common Linux serial symlinks first
        if let Ok(entries) = std::fs::read_dir("/dev/serial/by-id") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("Espressif") || name.contains("USB_JTAG_serial") || name.contains("ACM") {
                    if let Ok(path) = std::fs::canonicalize(entry.path()) {
                        return Some(path);
                    }
                }
            }
        }

        // Check common default ports
        for candidate in &["/dev/ttyACM0", "/dev/ttyACM1", "/dev/ttyUSB0"] {
            if Path::new(candidate).exists() {
                return Some(PathBuf::from(candidate));
            }
        }

        // Probe available ports from serialport crate
        if let Ok(ports) = serialport::available_ports() {
            for p in ports {
                if p.port_name.contains("ACM") || p.port_name.contains("usbmodem") {
                    return Some(PathBuf::from(p.port_name));
                }
            }
        }

        None
    }

    /// Transmit a mesh packet over serial CDC using length-prefixed binary framing (KTD2).
    /// Framing format: [0xAA, 0x55, 0x72, <114 wire bytes>] (117 bytes total).
    pub fn send_packet(&mut self, packet: &MeshPacket) -> io::Result<()> {
        let mut framed = [0u8; HOST_TO_DONGLE_TOTAL_LEN];
        framed[0..3].copy_from_slice(&HOST_TO_DONGLE_FRAME_HEADER);
        let wire_slice: &mut [u8; MeshPacket::WIRE_PAYLOAD_LEN] = (&mut framed[3..HOST_TO_DONGLE_TOTAL_LEN])
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Slice length mismatch"))?;
        packet.serialize_payload(wire_slice);

        self.port.write_all(&framed)?;
        self.port.flush()?;
        Ok(())
    }

    /// Poll incoming serial stream, returning extracted wire packets and informational log lines.
    /// Resilient against interleaved println! logs and capped at 1,024 bytes per line (R3).
    pub fn poll_stream(&mut self) -> (Vec<ReceivedWirePacket>, Vec<String>) {
        let mut packets = Vec::new();
        let mut logs = Vec::new();

        if let Ok(count) = self.port.bytes_to_read() {
            if count > 0 {
                let mut chunk = vec![0u8; (count as usize).min(1024)];
                if let Ok(n) = self.port.read(&mut chunk) {
                    if n > 0 {
                        self.read_buf.extend_from_slice(&chunk[..n]);

                        // Enforce 1,024 byte safety limit: if buffer exceeds limit without newline,
                        // drop bytes until the next newline or clear buffer
                        if self.read_buf.len() > DONGLE_TO_HOST_LINE_LIMIT {
                            if let Some(pos) = self.read_buf.iter().position(|&b| b == b'\n') {
                                self.read_buf.drain(..=pos);
                            } else {
                                self.read_buf.clear();
                            }
                        }

                        while let Some(pos) = self.read_buf.iter().position(|&b| b == b'\n') {
                            let line_bytes: Vec<u8> = self.read_buf.drain(..=pos).collect();
                            if let Ok(s) = std::str::from_utf8(&line_bytes) {
                                let trimmed = s.trim();
                                if !trimmed.is_empty() {
                                    if let Some(wire_pkt) = parse_pkt_line(trimmed) {
                                        packets.push(wire_pkt);
                                    } else {
                                        logs.push(trimmed.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (packets, logs)
    }

    /// Legacy compat method: return any complete text log lines from the dongle
    pub fn poll_dongle_lines(&mut self) -> Vec<String> {
        let (_pkts, logs) = self.poll_stream();
        logs
    }
}

/// Parses an ASCII line: `#PKT# <8-hex src_node_id> <228-hex wire_packet>`
pub fn parse_pkt_line(line: &str) -> Option<ReceivedWirePacket> {
    if !line.starts_with("#PKT# ") {
        return None;
    }

    let remainder = &line["#PKT# ".len()..];
    let mut parts = remainder.split_whitespace();
    let src_node_hex = parts.next()?;
    let wire_hex = parts.next()?;

    if parts.next().is_some() {
        return None; // Unexpected extra tokens
    }

    if src_node_hex.len() != 8 || wire_hex.len() != MeshPacket::WIRE_PAYLOAD_LEN * 2 {
        return None;
    }

    let src_node_id = u32::from_str_radix(src_node_hex, 16).ok()?;

    let mut wire_bytes = [0u8; MeshPacket::WIRE_PAYLOAD_LEN];
    for (i, chunk) in wire_hex.as_bytes().chunks(2).enumerate() {
        let byte_str = std::str::from_utf8(chunk).ok()?;
        wire_bytes[i] = u8::from_str_radix(byte_str, 16).ok()?;
    }

    let packet = MeshPacket::deserialize_payload(&wire_bytes);
    Some(ReceivedWirePacket {
        src_node_id,
        packet,
    })
}

/// Parses a dongle's station node ID from banner or telemetry log lines.
/// Examples:
/// - "Node ID: BEBD82B4 (MAC: [38, 44, BE, BD, 82, B4])"
/// - "[Dongle /dev/ttyACM1] [Node BEBD82B4] Uptime: 204s | ..."
pub fn parse_dongle_node_id(line: &str) -> Option<u32> {
    if let Some(pos) = line.find("[Node ") {
        let remainder = &line[pos + 6..];
        let hex_part = remainder.split(|c: char| c == ']' || c.is_whitespace()).next()?;
        if hex_part.len() == 8 {
            if let Ok(id) = u32::from_str_radix(hex_part, 16) {
                return Some(id);
            }
        }
    }

    if let Some(pos) = line.find("Node ID: ") {
        let remainder = &line[pos + 9..];
        let hex_part = remainder.split(|c: char| c == '(' || c.is_whitespace()).next()?;
        if hex_part.len() == 8 {
            if let Ok(id) = u32::from_str_radix(hex_part, 16) {
                return Some(id);
            }
        }
    }

    None
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use gibberish_protocol::{MeshHeader, DEFAULT_NETWORK_TAG, FLAG_CLIPBOARD};

    #[test]
    fn test_parse_dongle_node_id() {
        assert_eq!(
            parse_dongle_node_id("Node ID: BEBD82B4 (MAC: [38, 44, BE, BD, 82, B4])"),
            Some(0xBEBD82B4)
        );
        assert_eq!(
            parse_dongle_node_id("[Dongle /dev/ttyACM1] [Node BEBCE5B8] Uptime: 204s | Storage: MicroSdActive"),
            Some(0xBEBCE5B8)
        );
        assert_eq!(
            parse_dongle_node_id("[Node 12345678]"),
            Some(0x12345678)
        );
        assert_eq!(parse_dongle_node_id("Plain log line without node info"), None);
        assert_eq!(parse_dongle_node_id("[Node INVALID]"), None);
    }

    #[test]
    fn test_pkt_line_parsing() {
        let hdr = MeshHeader {
            network_tag: DEFAULT_NETWORK_TAG,
            msg_id: 0x12345678,
            chunk_idx: 1,
            total_chunks: 3,
            ttl: 5,
            hop_count: 1,
            flags: FLAG_CLIPBOARD,
        };
        let packet = MeshPacket {
            header: hdr,
            payload: [0x5A; 96],
        };
        let mut wire = [0u8; 114];
        packet.serialize_payload(&mut wire);

        let mut hex_str = String::new();
        for b in wire.iter() {
            hex_str.push_str(&format!("{:02X}", b));
        }

        let line = format!("#PKT# BEBCE5B8 {}", hex_str);
        let parsed = parse_pkt_line(&line).expect("Failed to parse valid #PKT# line");

        assert_eq!(parsed.src_node_id, 0xBEBCE5B8);
        assert_eq!(parsed.packet.header.msg_id, 0x12345678);
        assert_eq!(parsed.packet.header.chunk_idx, 1);
        assert_eq!(parsed.packet.header.total_chunks, 3);
        assert_eq!(parsed.packet.payload[0], 0x5A);
    }

    #[test]
    fn test_invalid_pkt_lines() {
        assert!(parse_pkt_line("not a packet line").is_none());
        assert!(parse_pkt_line("#PKT# BEBC SHORT_PAYLOAD").is_none());
        assert!(parse_pkt_line("#PKT# NOT_HEX 01020304").is_none());
        assert!(parse_pkt_line("#PKT# BEBCE5B8").is_none());
    }

    #[test]
    fn test_binary_transmit_framing() {
        let hdr = MeshHeader {
            network_tag: DEFAULT_NETWORK_TAG,
            msg_id: 999,
            chunk_idx: 0,
            total_chunks: 1,
            ttl: 7,
            hop_count: 0,
            flags: FLAG_CLIPBOARD,
        };
        let packet = MeshPacket {
            header: hdr,
            payload: [0x42; 96],
        };

        let mut framed = [0u8; HOST_TO_DONGLE_TOTAL_LEN];
        framed[0..3].copy_from_slice(&HOST_TO_DONGLE_FRAME_HEADER);
        let wire_slice: &mut [u8; MeshPacket::WIRE_PAYLOAD_LEN] = (&mut framed[3..HOST_TO_DONGLE_TOTAL_LEN])
            .try_into()
            .unwrap();
        packet.serialize_payload(wire_slice);

        assert_eq!(framed[0], 0xAA);
        assert_eq!(framed[1], 0x55);
        assert_eq!(framed[2], 0x72); // 114
        assert_eq!(framed.len(), 117);
    }
}
