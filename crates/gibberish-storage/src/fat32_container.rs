//! FAT32 circular append container (`CHUNKS.BIN`) and power-loss atomic framing (R8, R23, R26).

use crc32fast::Hasher;
use gibberish_protocol::{CIPHERTEXT_LEN, RECORD_COMMIT_MARKER, RECORD_MAGIC};

pub const RECORD_SIZE: usize = 116;
pub const SECTOR_SIZE: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum StorageError {
    DeviceError,
    TornWriteDetected,
    InvalidMagic,
    CrcMismatch,
    BufferTooSmall,
    SectorOutOfBounds,
}

/// Power-loss atomic record framing inside pre-allocated `CHUNKS.BIN` container (R23).
/// Layout:
/// - 0..4: Magic `GIBB` (4 bytes)
/// - 4..12: Sequence Number (8 bytes, big-endian)
/// - 12..14: Payload Length (2 bytes, big-endian, typically 96)
/// - 14..110: Ciphertext (96 bytes)
/// - 110..114: CRC32 checksum over bytes 0..110 (4 bytes, big-endian)
/// - 114..116: Commit marker `0xAA55` (2 bytes, big-endian)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRecord {
    pub sequence: u64,
    pub ciphertext: [u8; CIPHERTEXT_LEN],
}

impl ChunkRecord {
    pub const BYTE_LEN: usize = RECORD_SIZE;

    pub fn serialize(&self, out: &mut [u8; RECORD_SIZE]) {
        out[0..4].copy_from_slice(&RECORD_MAGIC);
        out[4..12].copy_from_slice(&self.sequence.to_be_bytes());
        out[12..14].copy_from_slice(&(CIPHERTEXT_LEN as u16).to_be_bytes());
        out[14..110].copy_from_slice(&self.ciphertext);

        // Compute CRC32 over magic, sequence, len, and ciphertext
        let mut hasher = Hasher::new();
        hasher.update(&out[0..110]);
        let crc = hasher.finalize();

        out[110..114].copy_from_slice(&crc.to_be_bytes());
        // Commit word is written last to seal the record
        out[114..116].copy_from_slice(&RECORD_COMMIT_MARKER.to_be_bytes());
    }

    pub fn deserialize(buf: &[u8; RECORD_SIZE]) -> Result<Self, StorageError> {
        // 1. Verify Magic
        if buf[0..4] != RECORD_MAGIC {
            return Err(StorageError::InvalidMagic);
        }

        // 2. Verify Commit marker (if missing, mid-write power loss occurred)
        let commit = u16::from_be_bytes(buf[114..116].try_into().unwrap());
        if commit != RECORD_COMMIT_MARKER {
            return Err(StorageError::TornWriteDetected);
        }

        // 3. Verify CRC32
        let expected_crc = u32::from_be_bytes(buf[110..114].try_into().unwrap());
        let mut hasher = Hasher::new();
        hasher.update(&buf[0..110]);
        let calculated_crc = hasher.finalize();

        if calculated_crc != expected_crc {
            return Err(StorageError::CrcMismatch);
        }

        let sequence = u64::from_be_bytes(buf[4..12].try_into().unwrap());
        let mut ciphertext = [0u8; CIPHERTEXT_LEN];
        ciphertext.copy_from_slice(&buf[14..110]);

        Ok(Self {
            sequence,
            ciphertext,
        })
    }
}

/// Ping-Pong Checkpoint sector for crash recovery in <15ms (R26).
/// Stored alternatively at LBA 1 and LBA 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointSector {
    pub sequence: u64,
    pub head_offset: u64,
    pub tail_offset: u64,
    pub total_records: u64,
}

impl CheckpointSector {
    pub const MAGIC: [u8; 4] = *b"GCPT";

    pub fn serialize(&self, sector: &mut [u8; SECTOR_SIZE]) {
        sector.fill(0);
        sector[0..4].copy_from_slice(&Self::MAGIC);
        sector[4..12].copy_from_slice(&self.sequence.to_be_bytes());
        sector[12..20].copy_from_slice(&self.head_offset.to_be_bytes());
        sector[20..28].copy_from_slice(&self.tail_offset.to_be_bytes());
        sector[28..36].copy_from_slice(&self.total_records.to_be_bytes());

        let mut hasher = Hasher::new();
        hasher.update(&sector[0..36]);
        let crc = hasher.finalize();
        sector[36..40].copy_from_slice(&crc.to_be_bytes());
        sector[510..512].copy_from_slice(&RECORD_COMMIT_MARKER.to_be_bytes());
    }

    pub fn deserialize(sector: &[u8; SECTOR_SIZE]) -> Result<Self, StorageError> {
        if sector[0..4] != Self::MAGIC {
            return Err(StorageError::InvalidMagic);
        }

        let commit = u16::from_be_bytes(sector[510..512].try_into().unwrap());
        if commit != RECORD_COMMIT_MARKER {
            return Err(StorageError::TornWriteDetected);
        }

        let expected_crc = u32::from_be_bytes(sector[36..40].try_into().unwrap());
        let mut hasher = Hasher::new();
        hasher.update(&sector[0..36]);
        if hasher.finalize() != expected_crc {
            return Err(StorageError::CrcMismatch);
        }

        let sequence = u64::from_be_bytes(sector[4..12].try_into().unwrap());
        let head_offset = u64::from_be_bytes(sector[12..20].try_into().unwrap());
        let tail_offset = u64::from_be_bytes(sector[20..28].try_into().unwrap());
        let total_records = u64::from_be_bytes(sector[28..36].try_into().unwrap());

        Ok(Self {
            sequence,
            head_offset,
            tail_offset,
            total_records,
        })
    }
}

/// Generic block device interface for SD card SPI access or host simulation.
pub trait BlockDevice {
    fn read_block(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), StorageError>;
    fn write_block(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), StorageError>;
}

/// Standard SD/MMC SPI Command 7-bit CRC generator with stop bit (polynomial x^7 + x^3 + 1).
pub fn crc7(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        for i in (0..8).rev() {
            let bit = (b >> i) & 1;
            let carry = (crc >> 7) & 1;
            crc <<= 1;
            if (carry ^ bit) != 0 {
                crc ^= 0x12;
            }
        }
    }
    crc | 1
}

use gibberish_protocol::StorageModeStatus;
use crate::sram_ring::SramRingBuffer;

/// Dual-mode storage manager managing SD card persistence and RAM-only fallback (R7, R8, R26, R28, R29).
pub struct DynamicStorageManager<D: BlockDevice> {
    device: Option<D>,
    mode: StorageModeStatus,
    current_sequence: u64,
    current_lba: u32,
    torn_writes_recovered: u32,
}

impl<D: BlockDevice> DynamicStorageManager<D> {
    pub fn new_ram_only() -> Self {
        Self {
            device: None,
            mode: StorageModeStatus::RamOnly,
            current_sequence: 0,
            current_lba: 10, // Chunk container begins after FAT32 reserved sectors
            torn_writes_recovered: 0,
        }
    }

    /// Initialize with an active block device, performing ping-pong checkpoint recovery across LBA 1 and LBA 2 (R26).
    pub fn new_with_device(mut dev: D) -> Self {
        let mut sector1 = [0u8; SECTOR_SIZE];
        let mut sector2 = [0u8; SECTOR_SIZE];

        let read1_res = dev.read_block(1, &mut sector1);
        let read2_res = dev.read_block(2, &mut sector2);

        // If both sector reads failed with I/O errors, fall back to RAM only
        if read1_res.is_err() && read2_res.is_err() {
            return Self {
                device: Some(dev),
                mode: StorageModeStatus::RamOnly,
                current_sequence: 0,
                current_lba: 10,
                torn_writes_recovered: 0,
            };
        }

        let ckpt1 = read1_res.ok().and_then(|()| CheckpointSector::deserialize(&sector1).ok());
        let ckpt2 = read2_res.ok().and_then(|()| CheckpointSector::deserialize(&sector2).ok());

        let mut torn_recovered = 0u32;
        let (seq, lba) = match (ckpt1, ckpt2) {
            (Some(c1), Some(c2)) => {
                if c1.sequence >= c2.sequence {
                    (c1.sequence, (c1.head_offset / (SECTOR_SIZE as u64)) as u32)
                } else {
                    (c2.sequence, (c2.head_offset / (SECTOR_SIZE as u64)) as u32)
                }
            }
            (Some(c1), None) => {
                // LBA 2 was invalid/torn; recovered from LBA 1
                torn_recovered = 1;
                (c1.sequence, (c1.head_offset / (SECTOR_SIZE as u64)) as u32)
            }
            (None, Some(c2)) => {
                // LBA 1 was invalid/torn; recovered from LBA 2
                torn_recovered = 1;
                (c2.sequence, (c2.head_offset / (SECTOR_SIZE as u64)) as u32)
            }
            (None, None) => {
                // Fresh card or no prior checkpoints
                (0, 10)
            }
        };

        Self {
            device: Some(dev),
            mode: StorageModeStatus::MicroSdActive,
            current_sequence: seq,
            current_lba: lba.max(10),
            torn_writes_recovered: torn_recovered,
        }
    }

    pub fn mode(&self) -> StorageModeStatus {
        self.mode
    }

    pub fn is_ram_only(&self) -> bool {
        self.mode == StorageModeStatus::RamOnly
    }

    pub fn torn_writes_recovered(&self) -> u32 {
        self.torn_writes_recovered
    }

    pub fn current_sequence(&self) -> u64 {
        self.current_sequence
    }

    pub fn current_lba(&self) -> u32 {
        self.current_lba
    }

    /// Bounded background flush from authoritative SRAM ring to MicroSD card container.
    /// Non-blocking to 5ms radio loop; flushes up to 2 chunks per tick and drops to RAM_ONLY
    /// seamlessly on card removal or write timeout without losing unwritten packets (R28, R29).
    pub fn flush_from_sram(&mut self, sram_ring: &mut SramRingBuffer) {
        if self.mode == StorageModeStatus::RamOnly || self.device.is_none() {
            return;
        }

        let dev = self.device.as_mut().unwrap();

        // Limit to at most 2 chunks per tick to prevent radio slot starvation
        for _ in 0..2 {
            let packet = match sram_ring.peek() {
                Some(p) => *p,
                None => break,
            };

            let next_seq = self.current_sequence.saturating_add(1);
            let record = ChunkRecord {
                sequence: next_seq,
                ciphertext: packet.payload,
            };

            let mut block_buf = [0u8; SECTOR_SIZE];
            record.serialize((&mut block_buf[0..ChunkRecord::BYTE_LEN]).try_into().unwrap());

            match dev.write_block(self.current_lba, &block_buf) {
                Ok(()) => {
                    self.current_sequence = next_seq;
                    self.current_lba = self.current_lba.saturating_add(1);
                    // Safely pop only after confirmed write
                    sram_ring.pop();

                    // Periodic ping-pong checkpointing (every 16 chunks)
                    if self.current_sequence.is_multiple_of(16) {
                        let ckpt_lba = if self.current_sequence.is_multiple_of(32) { 1 } else { 2 };
                        let ckpt = CheckpointSector {
                            sequence: self.current_sequence,
                            head_offset: (self.current_lba as u64) * (SECTOR_SIZE as u64),
                            tail_offset: 10 * (SECTOR_SIZE as u64),
                            total_records: self.current_sequence,
                        };
                        let mut ckpt_buf = [0u8; SECTOR_SIZE];
                        ckpt.serialize(&mut ckpt_buf);
                        let _ = dev.write_block(ckpt_lba, &ckpt_buf);
                    }
                }
                Err(_e) => {
                    // Hot-pull or hardware stall detected: fall back to RAM_ONLY seamlessly (R28)
                    self.mode = StorageModeStatus::RamOnly;
                    break;
                }
            }
        }
    }
}

