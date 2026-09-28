//! Authoritative 32 KB SRAM live packet ring buffer (R7, R28, KTD2).
//!
//! Provides a non-blocking 256-chunk circular queue in internal SRAM.
//! Live mesh routing and BLE streaming consume from this buffer directly with zero
//! storage dependence. If a MicroSD write task stalls, the oldest archival packets are
//! dropped with zero radio-loop stalls.

use gibberish_protocol::MeshPacket;

pub const SRAM_RING_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct SramRingBuffer {
    buffer: [Option<MeshPacket>; SRAM_RING_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    dropped_overflow: u32,
}

impl Default for SramRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SramRingBuffer {
    pub const fn new() -> Self {
        const INIT_SLOT: Option<MeshPacket> = None;
        Self {
            buffer: [INIT_SLOT; SRAM_RING_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            dropped_overflow: 0,
        }
    }

    /// Number of packets currently held in the ring buffer.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn is_full(&self) -> bool {
        self.count == SRAM_RING_CAPACITY
    }

    pub fn capacity(&self) -> usize {
        SRAM_RING_CAPACITY
    }

    pub fn dropped_count(&self) -> u32 {
        self.dropped_overflow
    }

    /// Push a packet into the live ring.
    /// If full, evicts the oldest packet (advancing tail) and increments dropped count.
    /// Returns `Some(evicted)` if an eviction occurred.
    pub fn push(&mut self, packet: MeshPacket) -> Option<MeshPacket> {
        let mut evicted = None;
        if self.is_full() {
            // Evict oldest
            evicted = self.buffer[self.tail].take();
            self.tail = (self.tail + 1) % SRAM_RING_CAPACITY;
            self.count -= 1;
            self.dropped_overflow = self.dropped_overflow.saturating_add(1);
        }

        self.buffer[self.head] = Some(packet);
        self.head = (self.head + 1) % SRAM_RING_CAPACITY;
        self.count += 1;

        evicted
    }

    /// Pop the oldest unconsumed packet from the ring.
    pub fn pop(&mut self) -> Option<MeshPacket> {
        if self.is_empty() {
            return None;
        }

        let packet = self.buffer[self.tail].take();
        self.tail = (self.tail + 1) % SRAM_RING_CAPACITY;
        self.count -= 1;
        packet
    }

    /// Peek at the oldest packet without popping.
    pub fn peek(&self) -> Option<&MeshPacket> {
        if self.is_empty() {
            None
        } else {
            self.buffer[self.tail].as_ref()
        }
    }

    /// Clear all packets from the ring.
    pub fn clear(&mut self) {
        for slot in self.buffer.iter_mut() {
            *slot = None;
        }
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }
}
