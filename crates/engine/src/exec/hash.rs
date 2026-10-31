//! Fx-style hasher with a finalizer (std's SipHash is too slow for byte-slice keys; no ahash dependency).
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    h: u64,
}

const K: u64 = 0x517c_c1b7_2722_0a95;

impl FxHasher {
    #[inline]
    fn add(&mut self, w: u64) {
        self.h = (self.h.rotate_left(5) ^ w).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            let mut b = [0u8; 8];
            b[..rem.len()].copy_from_slice(rem);
            self.add(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        let mut h = self.h;
        h ^= h >> 32;
        h = h.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        h ^ (h >> 29)
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxMap<K, V> = std::collections::HashMap<K, V, FxBuild>;
pub type FxSet<K> = std::collections::HashSet<K, FxBuild>;

/// Open-addressing (linear probing) map from an `i64` key to a dense group id. Slots hold `gid + 1`
/// (0 = empty); the keys themselves live in the caller's `keys[gid]` vector, so a group costs 4 bytes of
/// table plus 8 bytes of key and nothing is allocated per row. Load factor stays at or below 1/2.
pub struct IntMap {
    slots: Vec<u32>,
    shift: u32,
    count: usize,
}

impl Default for IntMap {
    fn default() -> Self {
        IntMap { slots: vec![0; 1 << 10], shift: 64 - 10, count: 0 }
    }
}

impl IntMap {
    #[inline]
    fn index(&self, k: i64) -> usize {
        // multiplicative hash; the top bits are the well-mixed ones
        ((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> self.shift) as usize
    }

    #[inline]
    pub fn find(&self, k: i64, keys: &[i64]) -> Option<u32> {
        let mask = self.slots.len() - 1;
        let mut i = self.index(k);
        loop {
            let s = self.slots[i];
            if s == 0 {
                return None;
            }
            if keys[(s - 1) as usize] == k {
                return Some(s - 1);
            }
            i = (i + 1) & mask;
        }
    }

    /// Insert group `gid`, whose key is already stored at `keys[gid]`. The key must not be present.
    pub fn insert(&mut self, gid: u32, keys: &[i64]) {
        if (self.count + 1) * 2 > self.slots.len() {
            self.grow(keys);
        }
        self.place(gid, keys[gid as usize]);
        self.count += 1;
    }

    #[inline]
    fn place(&mut self, gid: u32, k: i64) {
        let mask = self.slots.len() - 1;
        let mut i = self.index(k);
        while self.slots[i] != 0 {
            i = (i + 1) & mask;
        }
        self.slots[i] = gid + 1;
    }

    fn grow(&mut self, keys: &[i64]) {
        let old = std::mem::take(&mut self.slots);
        let bits = (old.len() * 2).trailing_zeros();
        self.slots = vec![0; old.len() * 2];
        self.shift = 64 - bits;
        for s in old {
            if s != 0 {
                self.place(s - 1, keys[(s - 1) as usize]);
            }
        }
    }
}
