//! Bounded-memory CSV reader for non-seekable input (stdin, pipes).
//!
//! A reader thread fills a bounded queue of pieces (`QUEUE_PIECES` x up to 1 MiB). The consumer appends
//! pieces to a buffer and cuts it into *blocks* that end after the last newline outside quotes; each block is
//! parsed by the ordinary `CsvReader`, and the partial last row carries over to the next block. Memory for the
//! input is therefore queue + one block + one partial row, independent of the input size. (Group tables and
//! join build sides are not part of this bound.)
//!
//! Exactness of the cut: every `"` byte in valid CSV is structural (`""` toggles twice, and a `"` inside an
//! unquoted field is rejected by the scanner), so the quote state at any byte is the parity of the quote bytes
//! before it. This is the same rule `CsvSource::plan_chunks` uses. For invalid input with a stray quote the
//! parity is wrong only after that quote, and the block containing the stray quote is parsed first, so the
//! scanner reports it at the same row as file mode does.
use super::{CsvReader, CsvSource, Data};
use crate::exec::Op;
use crate::storage::TableDef;
use crate::types::Batch;
use crate::{err, Result};
use memchr::memchr2;
use std::io::Read;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};

/// Pieces buffered between the reader thread and the parser.
pub const QUEUE_PIECES: usize = 16;
/// Largest piece the reader thread sends.
pub const PIECE_BYTES: usize = 1 << 20;
/// Default block target (bytes buffered before a block is cut), when `--chunk-bytes` is not given.
pub const DEFAULT_BLOCK_BYTES: usize = 4 << 20;
/// A single row longer than this is an error (bounds the carry-over buffer).
pub const MAX_ROW_BYTES: usize = 64 << 20;

/// The one non-seekable input of a query: which table it feeds and the reader (taken once).
pub struct StreamInput {
    /// Lower-cased table name, as in the catalog.
    pub table: String,
    reader: Mutex<Option<Box<dyn Read + Send>>>,
}

impl StreamInput {
    pub fn new(table: &str, reader: Box<dyn Read + Send>) -> StreamInput {
        StreamInput { table: table.to_ascii_lowercase(), reader: Mutex::new(Some(reader)) }
    }

    /// A stream with no data behind it. Used only to replay a failed query through the reference path to
    /// derive file-table error text; the empty stream produces no rows and no errors.
    pub fn replay(table: &str) -> StreamInput {
        StreamInput { table: table.to_ascii_lowercase(), reader: Mutex::new(None) }
    }

    pub fn take(&self) -> Option<Box<dyn Read + Send>> {
        self.reader.lock().unwrap().take()
    }
}

type Piece = std::io::Result<Vec<u8>>;

fn spawn_reader(mut r: Box<dyn Read + Send>, piece: usize) -> Receiver<Piece> {
    let (tx, rx) = sync_channel::<Piece>(QUEUE_PIECES);
    std::thread::spawn(move || loop {
        let mut v = vec![0u8; piece];
        let mut n = 0;
        while n < piece {
            match r.read(&mut v[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
        if n == 0 {
            return;
        }
        v.truncate(n);
        // The consumer hung up (LIMIT satisfied, error): stop reading.
        if tx.send(Ok(v)).is_err() || n < piece {
            return;
        }
    });
    rx
}

/// Cuts a stream of pieces into blocks of whole rows.
pub struct BlockSplitter {
    rx: Receiver<Piece>,
    buf: Vec<u8>,
    /// Bytes of `buf` already examined.
    scanned: usize,
    /// Quote parity at `scanned` (state at a row start is `false`).
    inside: bool,
    /// End of the last complete row in `buf` (0 = none yet).
    cut: usize,
    target: usize,
    max_row: usize,
    eof: bool,
}

impl BlockSplitter {
    pub fn new(reader: Box<dyn Read + Send>, target: usize) -> BlockSplitter {
        let target = target.max(1);
        Self::with_limits(reader, target, PIECE_BYTES.min(target), MAX_ROW_BYTES)
    }

    pub fn with_limits(reader: Box<dyn Read + Send>, target: usize, piece: usize, max_row: usize) -> BlockSplitter {
        BlockSplitter {
            rx: spawn_reader(reader, piece.max(1)),
            buf: Vec::new(),
            scanned: 0,
            inside: false,
            cut: 0,
            target,
            max_row,
            eof: false,
        }
    }

    /// Next block of whole rows (the final block may end without a newline), or `None` at end of input.
    pub fn next_block(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                self.scanned = 0;
                self.cut = 0;
                return Ok(Some(std::mem::take(&mut self.buf)));
            }
            if self.cut > 0 && self.buf.len() >= self.target {
                let tail = self.buf.split_off(self.cut);
                let block = std::mem::replace(&mut self.buf, tail);
                self.scanned -= self.cut;
                self.cut = 0;
                return Ok(Some(block));
            }
            match self.rx.recv() {
                Err(_) => self.eof = true,
                Ok(Err(e)) => return Err(err!("<stdin>: read error: {e}")),
                Ok(Ok(piece)) => {
                    self.buf.extend_from_slice(&piece);
                    self.scan();
                    if self.buf.len() - self.cut > self.max_row {
                        return Err(err!("<stdin>: a row is longer than {} MiB", self.max_row >> 20));
                    }
                }
            }
        }
    }

    fn scan(&mut self) {
        let mut p = self.scanned;
        while let Some(k) = memchr2(b'"', b'\n', &self.buf[p..]) {
            let at = p + k;
            if self.buf[at] == b'"' {
                self.inside = !self.inside;
            } else if !self.inside {
                self.cut = at + 1;
            }
            p = at + 1;
        }
        self.scanned = self.buf.len();
    }
}

/// Scan operator over a stream: header from the first block, then one `CsvReader` per block.
pub struct StreamScan {
    table: TableDef,
    cols: Vec<usize>,
    reference: bool,
    splitter: BlockSplitter,
    src: Option<CsvSource>,
    reader: Option<CsvReader>,
    rows: u64,
}

impl StreamScan {
    pub fn new(
        input: Box<dyn Read + Send>,
        table: TableDef,
        cols: Vec<usize>,
        reference: bool,
        block_bytes: Option<usize>,
    ) -> StreamScan {
        StreamScan {
            table,
            cols,
            reference,
            splitter: BlockSplitter::new(input, block_bytes.unwrap_or(DEFAULT_BLOCK_BYTES)),
            src: None,
            reader: None,
            rows: 0,
        }
    }
}

impl Op for StreamScan {
    fn next(&mut self) -> Result<Option<Batch>> {
        loop {
            if let Some(r) = &mut self.reader {
                if let Some(b) = r.next_batch()? {
                    crate::progress::add_rows_in(b.len);
                    return Ok(Some(b));
                }
                self.rows = r.rows();
                self.reader = None;
            }
            let Some(block) = self.splitter.next_block()? else {
                if self.src.is_none() {
                    return Err(err!("<stdin>: empty file (missing header row)"));
                }
                return Ok(None);
            };
            let data: Data = Arc::new(block);
            let len = AsRef::<[u8]>::as_ref(&*data).len();
            match &self.src {
                None => {
                    let src = CsvSource::from_data(
                        data.clone(),
                        &self.table,
                        &self.cols,
                        self.reference,
                        "<stdin>".to_string(),
                    )?;
                    self.reader = Some(src.reader(src.data_start(), len));
                    self.src = Some(src);
                }
                Some(src) => self.reader = Some(src.reader_on(data, 0, len, self.rows)),
            }
        }
    }
}

/// Always-empty operator (replay of a stream scan, see `StreamInput::replay`).
pub struct EmptyScan;

impl Op for EmptyScan {
    fn next(&mut self) -> Result<Option<Batch>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(data: &[u8], target: usize, piece: usize) -> Vec<Vec<u8>> {
        let mut s = BlockSplitter::with_limits(Box::new(std::io::Cursor::new(data.to_vec())), target, piece, MAX_ROW_BYTES);
        let mut v = vec![];
        while let Some(b) = s.next_block().unwrap() {
            v.push(b);
        }
        v
    }

    #[test]
    fn blocks_concatenate_and_cut_outside_quotes() {
        let data = b"h\n\"a\nb\",1\nx,\"q\"\"\n\"\nlast,2";
        for target in 1..40 {
            for piece in 1..12 {
                let bs = blocks(data, target, piece);
                assert_eq!(bs.concat(), data, "target {target} piece {piece}");
                for b in &bs[..bs.len() - 1] {
                    assert_eq!(b.last(), Some(&b'\n'));
                    // every non-final block holds an even number of quote bytes
                    assert_eq!(b.iter().filter(|&&c| c == b'"').count() % 2, 0);
                }
            }
        }
    }

    #[test]
    fn long_row_is_an_error() {
        let mut s = BlockSplitter::with_limits(Box::new(std::io::Cursor::new(vec![b'x'; 5 << 20])), 1 << 20, 1 << 20, 2 << 20);
        let e = loop {
            match s.next_block() {
                Err(e) => break e,
                Ok(Some(_)) => {}
                Ok(None) => panic!("expected an error"),
            }
        };
        assert!(e.0.contains("longer than 2 MiB"), "{}", e.0);
    }
}
