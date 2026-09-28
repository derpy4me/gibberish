//! USB CDC-ACM Serial transport to LilyGO T-Dongle-C5 (R1, R3, R4, KTD2).

use gibberish_protocol::{decode_cdc_frame, CDC_FRAME_MAGIC, ClosedTelemetry, MeshPacket};
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
            .map_err(|e| io::Error::other(e.to_string()))?;

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
    /// Resilient against interleaved binary CDC frames, println! logs, and capped at buffer limits.
    pub fn poll_stream(&mut self) -> (Vec<ReceivedWirePacket>, Vec<String>) {
        let mut packets = Vec::new();
        let mut logs = Vec::new();

        if let Ok(count) = self.port.bytes_to_read() {
            if count > 0 {
                let mut chunk = vec![0u8; (count as usize).min(1024)];
                if let Ok(n) = self.port.read(&mut chunk) {
                    if n > 0 {
                        self.read_buf.extend_from_slice(&chunk[..n]);

                        let (pkts, telems, log_lines) = parse_cdc_stream_sliding_window(&mut self.read_buf);
                        packets.extend(pkts);
                        logs.extend(log_lines);
                        for telem in telems {
                            let storage_str = match telem.storage_mode {
                                gibberish_protocol::StorageModeStatus::MicroSdActive => "SD ACTIVE",
                                gibberish_protocol::StorageModeStatus::RamOnly => "RAM ONLY",
                            };
                            logs.push(format!(
                                "[Telemetry RX] Tier: PROD, Storage: {}, Uptime: {}s, SRAM: {}/256, Drops: {}, RX: {}, TX: {}, RSSI: {} dBm, LQI: 255",
                                storage_str,
                                telem.uptime_secs,
                                telem.sram_ring_used,
                                telem.dropped_count,
                                telem.rx_packet_count,
                                telem.tx_packet_count,
                                telem.last_rssi,
                            ));
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

/// Scans a byte buffer for CRC16-CCITT validated CDC frames and/or ASCII lines using a sliding window.
/// Handles interleaved plaintext logs, corrupt frames, noise, and valid framed Postcard or MeshPacket binaries.
pub fn parse_cdc_stream_sliding_window(
    read_buf: &mut Vec<u8>,
) -> (Vec<ReceivedWirePacket>, Vec<ClosedTelemetry>, Vec<String>) {
    let mut packets = Vec::new();
    let mut telemetries = Vec::new();
    let mut logs = Vec::new();

    if read_buf.len() > DONGLE_TO_HOST_LINE_LIMIT * 4 {
        let drain_len = read_buf.len().saturating_sub(DONGLE_TO_HOST_LINE_LIMIT);
        read_buf.drain(..drain_len);
    }

    loop {
        if read_buf.is_empty() {
            break;
        }

        // 1. Check if buffer starts with binary CDC magic [0xAA, 0x55]
        if read_buf.len() >= 2 && read_buf[0..2] == CDC_FRAME_MAGIC {
            match decode_cdc_frame(read_buf) {
                Ok(Some((consumed, payload))) => {
                    if payload.len() == MeshPacket::WIRE_PAYLOAD_LEN {
                        let pkt = MeshPacket::deserialize_payload(payload.try_into().unwrap());
                        packets.push(ReceivedWirePacket {
                            src_node_id: 0,
                            packet: pkt,
                        });
                    } else if let Ok(telem) = postcard::from_bytes::<ClosedTelemetry>(payload) {
                        telemetries.push(telem);
                    }
                    read_buf.drain(..consumed);
                    continue;
                }
                Ok(None) => {
                    // Frame header valid or partially received, waiting for more bytes
                    break;
                }
                Err(_) => {
                    // CRC mismatch or bad magic: false-positive magic or corrupted frame.
                    // Slide window forward by 1 byte to resynchronize on the next valid frame.
                    read_buf.drain(..1);
                    continue;
                }
            }
        }

        // 2. Check for newline-delimited ASCII log line
        if let Some(nl_pos) = read_buf.iter().position(|&b| b == b'\n') {
            if let Some(magic_pos) = read_buf[..nl_pos]
                .windows(2)
                .position(|w| w == CDC_FRAME_MAGIC)
            {
                if magic_pos > 0 {
                    let line_bytes: Vec<u8> = read_buf.drain(..magic_pos).collect();
                    if let Ok(s) = std::str::from_utf8(&line_bytes) {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            logs.push(trimmed.to_string());
                        }
                    }
                }
                continue;
            }

            let line_bytes: Vec<u8> = read_buf.drain(..=nl_pos).collect();
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
            continue;
        }

        // 3. No newline. Check if CDC_FRAME_MAGIC appears anywhere in the buffer.
        if let Some(magic_pos) = read_buf.windows(2).position(|w| w == CDC_FRAME_MAGIC) {
            if magic_pos > 0 {
                let text_bytes: Vec<u8> = read_buf.drain(..magic_pos).collect();
                if let Ok(s) = std::str::from_utf8(&text_bytes) {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        logs.push(trimmed.to_string());
                    }
                }
            }
            continue;
        }

        // Wait for more data
        break;
    }

    (packets, telemetries, logs)
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

    #[test]
    fn test_framed_postcard_crc16_validation_and_resync() {
        use gibberish_protocol::{encode_cdc_frame, ClosedTelemetry, DiagnosticEventCode, StorageModeStatus};

        let mut telem = ClosedTelemetry::new();
        telem.uptime_secs = 1234;
        telem.rx_packet_count = 50;
        telem.tx_packet_count = 25;
        telem.dropped_count = 1;
        telem.sram_ring_used = 12;
        telem.storage_mode = StorageModeStatus::MicroSdActive;
        telem.last_event = DiagnosticEventCode::RadioRxOk;
        telem.last_rssi = -30;

        let mut postcard_buf = [0u8; 64];
        let postcard_slice = postcard::to_slice(&telem, &mut postcard_buf).unwrap();

        let mut valid_frame = [0u8; 80];
        let valid_frame_len = encode_cdc_frame(postcard_slice, &mut valid_frame).unwrap();

        // 1. Valid frame parses cleanly
        let mut buf = valid_frame[..valid_frame_len].to_vec();
        let (pkts, telems, _logs) = parse_cdc_stream_sliding_window(&mut buf);
        assert!(buf.is_empty());
        assert_eq!(pkts.len(), 0);
        assert_eq!(telems.len(), 1);
        assert_eq!(telems[0].uptime_secs, 1234);
        assert_eq!(telems[0].storage_mode, StorageModeStatus::MicroSdActive);

        // 2. Stream with leading noise and trailing text logs
        let mut stream = Vec::new();
        stream.extend_from_slice(b"random garbage noise before frame\n");
        stream.extend_from_slice(&valid_frame[..valid_frame_len]);
        stream.extend_from_slice(b"another line of text after frame\n");

        let (_pkts, telems, logs) = parse_cdc_stream_sliding_window(&mut stream);
        assert!(stream.is_empty());
        assert_eq!(telems.len(), 1);
        assert_eq!(telems[0].uptime_secs, 1234);
        assert!(logs.iter().any(|l| l.contains("random garbage")));
        assert!(logs.iter().any(|l| l.contains("another line")));

        // 3. Corrupted CRC frame followed by valid frame (sliding-window resynchronization)
        let mut corrupt_frame = valid_frame[..valid_frame_len].to_vec();
        corrupt_frame[5] ^= 0xFF; // Invalidate CRC

        let mut stream_corrupt = Vec::new();
        stream_corrupt.extend_from_slice(&corrupt_frame);
        stream_corrupt.extend_from_slice(&valid_frame[..valid_frame_len]);

        let (_pkts, telems, _logs) = parse_cdc_stream_sliding_window(&mut stream_corrupt);
        assert!(stream_corrupt.is_empty());
        assert_eq!(telems.len(), 1);
        assert_eq!(telems[0].uptime_secs, 1234);
    }
}
