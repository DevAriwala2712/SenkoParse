//! Helpers shared by the stream tests.
use std::io::Read;

/// A reader that returns 1..=`max` random bytes per `read` call (never more than asked), like a slow pipe.
pub struct Dribble {
    data: Vec<u8>,
    pos: usize,
    state: u64,
    max: u64,
}

impl Dribble {
    pub fn new(data: &[u8], seed: u64) -> Dribble {
        Dribble { data: data.to_vec(), pos: 0, state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, max: 300 }
    }
    /// One byte per `read`.
    pub fn bytewise(data: &[u8]) -> Dribble {
        Dribble { max: 1, ..Dribble::new(data, 1) }
    }
}

impl Read for Dribble {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let want = (1 + self.state % self.max) as usize;
        let n = want.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
