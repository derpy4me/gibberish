#![no_std]

pub mod fat32_container;
pub mod sram_ring;

pub use fat32_container::{
    crc7, BlockDevice, CheckpointSector, ChunkRecord, DynamicStorageManager, StorageError,
    RECORD_SIZE, SECTOR_SIZE,
};
pub use sram_ring::{SramRingBuffer, SRAM_RING_CAPACITY};

