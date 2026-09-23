//! Sliding Bloom Filter and LRU cache for 802.15.4 mesh deduplication (R16).
//!
//! Prevents packet re-broadcast storms without dynamic heap allocation.
//! Sized for p < 0.01 false positive rate over 1,024 recent packets.

pub const BLOOM_BITS: usize = 10240; // 1280 bytes
pub const BLOOM_WORDS: usize = BLOOM_BITS / 64; // 160 u64s
pub const EPOCH_THRESHOLD: usize = 512;
pub const LRU_SIZE: usize = 64;

pub struct SlidingBloomFilter {
    active_epoch: [u64; BLOOM_WORDS],
    previous_epoch: [u64; BLOOM_WORDS],
    entries_in_epoch: usize,
    recent_lru: [(u32, u8); LRU_SIZE],
    lru_head: usize,
}

impl SlidingBloomFilter {
    pub const fn new() -> Self {
        Self {
            active_epoch: [0u64; BLOOM_WORDS],
            previous_epoch: [0u64; BLOOM_WORDS],
            entries_in_epoch: 0,
            recent_lru: [(0, 0); LRU_SIZE],
            lru_head: 0,
        }
    }

    /// Two-hash combination generating k=7 independent bit positions: h_i = (h1 + i * h2) % BLOOM_BITS
    fn hash_coords(msg_id: u32, chunk_idx: u8) -> (u32, u32) {
        // Simple fast mixing without heap (Murmur/FNV style)
        let mut h1 = msg_id.wrapping_mul(0x85ebca6b) ^ ((chunk_idx as u32).wrapping_mul(0xc2b2ae35));
        h1 = (h1 ^ (h1 >> 16)).wrapping_mul(0x85ebca6b);

        let mut h2 = msg_id.wrapping_mul(0xcc9e2d51) ^ ((chunk_idx as u32).wrapping_mul(0x1b873593));
        h2 = (h2 ^ (h2 >> 13)).wrapping_mul(0x5bd1e995);
        if h2 == 0 {
            h2 = 1;
        }
        (h1, h2)
    }

    /// Check if packet has likely been seen before.
    pub fn contains(&self, msg_id: u32, chunk_idx: u8) -> bool {
        // Fast path: check recent LRU
        for i in 0..LRU_SIZE {
            if self.recent_lru[i] == (msg_id, chunk_idx) {
                return true;
            }
        }

        let (h1, h2) = Self::hash_coords(msg_id, chunk_idx);
        for i in 0..7 {
            let bit_idx = (h1.wrapping_add(i * h2) as usize) % BLOOM_BITS;
            let word_idx = bit_idx / 64;
            let bit_pos = bit_idx % 64;
            let in_active = (self.active_epoch[word_idx] & (1u64 << bit_pos)) != 0;
            let in_prev = (self.previous_epoch[word_idx] & (1u64 << bit_pos)) != 0;
            if !in_active && !in_prev {
                return false;
            }
        }
        true
    }

    /// Insert a packet into the filter.
    pub fn insert(&mut self, msg_id: u32, chunk_idx: u8) {
        // Add to LRU
        self.recent_lru[self.lru_head] = (msg_id, chunk_idx);
        self.lru_head = (self.lru_head + 1) % LRU_SIZE;

        // Check for epoch turnover
        if self.entries_in_epoch >= EPOCH_THRESHOLD {
            self.previous_epoch = self.active_epoch;
            self.active_epoch = [0u64; BLOOM_WORDS];
            self.entries_in_epoch = 0;
        }

        let (h1, h2) = Self::hash_coords(msg_id, chunk_idx);
        for i in 0..7 {
            let bit_idx = (h1.wrapping_add(i * h2) as usize) % BLOOM_BITS;
            let word_idx = bit_idx / 64;
            let bit_pos = bit_idx % 64;
            self.active_epoch[word_idx] |= 1u64 << bit_pos;
        }
        self.entries_in_epoch += 1;
    }
}
