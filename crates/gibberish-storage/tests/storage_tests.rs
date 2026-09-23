use gibberish_protocol::{MeshHeader, MeshPacket, CIPHERTEXT_LEN};
use gibberish_storage::fat32_container::{
    BlockDevice, CheckpointSector, ChunkRecord, StorageError, RECORD_SIZE, SECTOR_SIZE,
};
use gibberish_storage::sram_ring::SramRingBuffer;

#[test]
fn test_sram_ring_buffer_fifo_and_eviction() {
    let mut ring = SramRingBuffer::new();
    assert!(ring.is_empty());
    assert_eq!(ring.len(), 0);

    let dummy_packet = |id: u32| {
        let header = MeshHeader {
            network_tag: 0x1234,
            msg_id: id,
            chunk_idx: 0,
            total_chunks: 1,
            ttl: 3,
            hop_count: 0,
            flags: 0,
        };
        MeshPacket {
            header,
            payload: [0u8; CIPHERTEXT_LEN],
        }
    };

    // Fill to capacity (256 items)
    for i in 0..256 {
        let evicted = ring.push(dummy_packet(i));
        assert!(evicted.is_none());
    }

    assert!(ring.is_full());
    assert_eq!(ring.len(), 256);
    assert_eq!(ring.dropped_count(), 0);

    // Push 257th item: should evict item 0
    let evicted = ring.push(dummy_packet(256));
    assert!(evicted.is_some());
    assert_eq!(evicted.unwrap().header.msg_id, 0);
    assert_eq!(ring.dropped_count(), 1);
    assert_eq!(ring.len(), 256);

    // Peek should show item 1
    assert_eq!(ring.peek().unwrap().header.msg_id, 1);

    // Pop should return item 1
    let popped = ring.pop().unwrap();
    assert_eq!(popped.header.msg_id, 1);
    assert_eq!(ring.len(), 255);
}

#[test]
fn test_chunk_record_power_loss_framing_and_corruption() {
    let mut ciphertext = [0u8; CIPHERTEXT_LEN];
    for (i, b) in ciphertext.iter_mut().enumerate() {
        *b = (i * 3) as u8;
    }

    let record = ChunkRecord {
        sequence: 1001,
        ciphertext,
    };

    let mut encoded = [0u8; RECORD_SIZE];
    record.serialize(&mut encoded);

    // Valid decode
    let decoded = ChunkRecord::deserialize(&encoded).expect("deserialization failed");
    assert_eq!(record, decoded);

    // Simulate mid-write power loss: commit word missing / zeroed
    let mut torn_write = encoded;
    torn_write[114] = 0x00;
    torn_write[115] = 0x00;
    let err = ChunkRecord::deserialize(&torn_write);
    assert_eq!(err, Err(StorageError::TornWriteDetected));

    // Simulate bit flip corruption: CRC fails
    let mut corrupted = encoded;
    corrupted[20] ^= 0x01;
    let err_crc = ChunkRecord::deserialize(&corrupted);
    assert_eq!(err_crc, Err(StorageError::CrcMismatch));
}

#[test]
fn test_checkpoint_sector_serialization_and_recovery() {
    let checkpoint = CheckpointSector {
        sequence: 42,
        head_offset: 512000,
        tail_offset: 1024,
        total_records: 5000,
    };

    let mut sector = [0u8; SECTOR_SIZE];
    checkpoint.serialize(&mut sector);

    let recovered = CheckpointSector::deserialize(&sector).expect("checkpoint recovery failed");
    assert_eq!(checkpoint, recovered);

    // Corrupted commit marker
    sector[511] = 0x00;
    assert_eq!(
        CheckpointSector::deserialize(&sector),
        Err(StorageError::TornWriteDetected)
    );
}

// In-memory mock block device for testing
#[derive(Clone)]
struct MockBlockDevice {
    blocks: [[u8; SECTOR_SIZE]; 128],
}

impl MockBlockDevice {
    fn new() -> Self {
        Self {
            blocks: [[0u8; SECTOR_SIZE]; 128],
        }
    }
}

impl BlockDevice for MockBlockDevice {
    fn read_block(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), StorageError> {
        if (lba as usize) < self.blocks.len() {
            buf.copy_from_slice(&self.blocks[lba as usize]);
            Ok(())
        } else {
            Err(StorageError::SectorOutOfBounds)
        }
    }

    fn write_block(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), StorageError> {
        if (lba as usize) < self.blocks.len() {
            self.blocks[lba as usize].copy_from_slice(buf);
            Ok(())
        } else {
            Err(StorageError::SectorOutOfBounds)
        }
    }
}

#[test]
fn test_mock_block_device_read_write() {
    let mut dev = MockBlockDevice::new();
    let sector = [0x5Au8; SECTOR_SIZE];
    dev.write_block(5, &sector).unwrap();

    let mut read_back = [0u8; SECTOR_SIZE];
    dev.read_block(5, &mut read_back).unwrap();
    assert_eq!(sector, read_back);

    assert_eq!(dev.read_block(999, &mut read_back), Err(StorageError::SectorOutOfBounds));
}

#[test]
fn test_crc7_vectors() {
    use gibberish_storage::crc7;

    // CMD0: [0x40, 0x00, 0x00, 0x00, 0x00] -> Expected 0x95
    assert_eq!(crc7(&[0x40, 0x00, 0x00, 0x00, 0x00]), 0x95);

    // CMD8: [0x48, 0x00, 0x00, 0x01, 0xAA] -> Expected 0x87
    assert_eq!(crc7(&[0x48, 0x00, 0x00, 0x01, 0xAA]), 0x87);

    // CMD17: [0x51, 0x00, 0x00, 0x00, 0x00] (READ_SINGLE_BLOCK LBA 0)
    let cmd17_crc = crc7(&[0x51, 0x00, 0x00, 0x00, 0x00]);
    assert_eq!(cmd17_crc & 0x01, 1); // Stop bit must be 1
}

#[test]
fn test_dynamic_storage_manager_ping_pong_recovery() {
    use gibberish_protocol::StorageModeStatus;
    use gibberish_storage::DynamicStorageManager;

    let mut dev = MockBlockDevice::new();

    // Case 1: Fresh device (no checkpoints)
    let sm_fresh = DynamicStorageManager::new_with_device(dev.clone());
    assert_eq!(sm_fresh.mode(), StorageModeStatus::MicroSdActive);
    assert_eq!(sm_fresh.current_sequence(), 0);
    assert_eq!(sm_fresh.current_lba(), 10);
    assert_eq!(sm_fresh.torn_writes_recovered(), 0);

    // Case 2: LBA 1 has valid checkpoint (seq 10), LBA 2 has torn write
    let ckpt1 = CheckpointSector {
        sequence: 10,
        head_offset: 512 * 20,
        tail_offset: 512 * 10,
        total_records: 10,
    };
    let mut s1 = [0u8; SECTOR_SIZE];
    ckpt1.serialize(&mut s1);
    dev.write_block(1, &s1).unwrap();

    let s2_torn = [0xFFu8; SECTOR_SIZE]; // Garbage / torn
    dev.write_block(2, &s2_torn).unwrap();

    let sm_rec1 = DynamicStorageManager::new_with_device(dev.clone());
    assert_eq!(sm_rec1.mode(), StorageModeStatus::MicroSdActive);
    assert_eq!(sm_rec1.current_sequence(), 10);
    assert_eq!(sm_rec1.current_lba(), 20);
    assert_eq!(sm_rec1.torn_writes_recovered(), 1);

    // Case 3: LBA 2 has newer valid checkpoint (seq 15), LBA 1 has seq 10
    let ckpt2 = CheckpointSector {
        sequence: 15,
        head_offset: 512 * 25,
        tail_offset: 512 * 10,
        total_records: 15,
    };
    let mut s2 = [0u8; SECTOR_SIZE];
    ckpt2.serialize(&mut s2);
    dev.write_block(2, &s2).unwrap();

    let sm_rec2 = DynamicStorageManager::new_with_device(dev.clone());
    assert_eq!(sm_rec2.mode(), StorageModeStatus::MicroSdActive);
    assert_eq!(sm_rec2.current_sequence(), 15);
    assert_eq!(sm_rec2.current_lba(), 25);
    assert_eq!(sm_rec2.torn_writes_recovered(), 0);
}

#[test]
fn test_dynamic_storage_manager_bounded_flush_and_peek() {
    use gibberish_protocol::{MeshHeader, MeshPacket};
    use gibberish_storage::DynamicStorageManager;

    let dev = MockBlockDevice::new();
    let mut sm = DynamicStorageManager::new_with_device(dev);
    let mut ring = SramRingBuffer::new();

    let dummy_packet = |id: u32| {
        MeshPacket {
            header: MeshHeader {
                network_tag: 0x4749424245524953,
                msg_id: id,
                chunk_idx: 0,
                total_chunks: 1,
                ttl: 3,
                hop_count: 0,
                flags: 0,
            },
            payload: [(id & 0xFF) as u8; CIPHERTEXT_LEN],
        }
    };

    // Push 5 packets
    for i in 0..5 {
        ring.push(dummy_packet(i));
    }
    assert_eq!(ring.len(), 5);

    // Flush 1 tick: bounded flush should write at most 2 chunks
    sm.flush_from_sram(&mut ring);
    assert_eq!(ring.len(), 3);
    assert_eq!(sm.current_sequence(), 2);
    assert_eq!(sm.current_lba(), 12);

    // Flush 2nd tick: 2 more chunks
    sm.flush_from_sram(&mut ring);
    assert_eq!(ring.len(), 1);
    assert_eq!(sm.current_sequence(), 4);

    // Flush 3rd tick: remaining 1 chunk
    sm.flush_from_sram(&mut ring);
    assert_eq!(ring.len(), 0);
    assert_eq!(sm.current_sequence(), 5);
}

