//! Allocation trace format v0 (`.ratrace`).
//!
//! Traces are recorded from real workloads (spacedb, rusty_h264, FFAI, …) by a
//! shim allocator and replayed for the G2 differential gate and for timing.
//! **Addresses never appear in a trace** — the recorder maps runtime pointers to
//! dense sequential ids, so a trace is deterministic, diffable, and replayable
//! against any allocator. Files are content-hash versioned in `corpus/traces/`.
//!
//! ## Layout (all little-endian)
//!
//! Header (16 bytes): magic `b"RATRACE0"`, format version `u32`, reserved `u32`.
//! Then fixed 32-byte records:
//!
//! | field | type | meaning |
//! |---|---|---|
//! | `op` | `u8` | see [`Op`] |
//! | `align_log2` | `u8` | requested alignment as log2; 0 = natural (≤ 16) |
//! | `thread` | `u16` | dense thread index assigned by the recorder |
//! | `_reserved` | `u32` | zero |
//! | `size` | `u64` | requested size (0 where n/a, e.g. `Free`) |
//! | `ptr` | `u64` | block id produced by this op (0 where n/a) |
//! | `old_ptr` | `u64` | block id consumed by this op (`Free`, `Realloc`) |

use std::io::{self, Read, Write};

/// File magic: `RATRACE0`.
pub const MAGIC: [u8; 8] = *b"RATRACE0";
/// Current format version.
pub const FORMAT_VERSION: u32 = 0;
/// Size of one encoded record in bytes.
pub const RECORD_SIZE: usize = 32;

/// Trace operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Op {
    /// `malloc`-family: allocate `size` bytes, not zeroed → `ptr`.
    Malloc = 0,
    /// `zalloc`/`calloc`-family: allocate `size` bytes, zeroed → `ptr`.
    Zalloc = 1,
    /// `free(old_ptr)`.
    Free = 2,
    /// `realloc(old_ptr, size)` → `ptr`. Zero-preserving and aligned variants
    /// get their own ops when the recorder learns them (M5); v0 stays minimal.
    Realloc = 3,
    /// Thread `thread` started (first op observed on it).
    ThreadStart = 4,
    /// Thread `thread` exited (drives abandonment in replay).
    ThreadEnd = 5,
}

impl Op {
    /// Decode an op byte.
    pub fn from_u8(v: u8) -> Option<Op> {
        match v {
            0 => Some(Op::Malloc),
            1 => Some(Op::Zalloc),
            2 => Some(Op::Free),
            3 => Some(Op::Realloc),
            4 => Some(Op::ThreadStart),
            5 => Some(Op::ThreadEnd),
            _ => None,
        }
    }
}

/// One trace record. `ptr`/`old_ptr` are recorder-assigned block ids, never
/// addresses (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// Operation kind.
    pub op: Op,
    /// Requested alignment as log2 (0 = natural alignment ≤ 16 bytes).
    pub align_log2: u8,
    /// Dense thread index.
    pub thread: u16,
    /// Requested size in bytes; 0 where not applicable.
    pub size: u64,
    /// Block id produced by this op; 0 where not applicable.
    pub ptr: u64,
    /// Block id consumed by this op; 0 where not applicable.
    pub old_ptr: u64,
}

impl Record {
    /// Encode into the fixed 32-byte wire form.
    pub fn encode(&self) -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        b[0] = self.op as u8;
        b[1] = self.align_log2;
        b[2..4].copy_from_slice(&self.thread.to_le_bytes());
        // b[4..8] reserved, zero
        b[8..16].copy_from_slice(&self.size.to_le_bytes());
        b[16..24].copy_from_slice(&self.ptr.to_le_bytes());
        b[24..32].copy_from_slice(&self.old_ptr.to_le_bytes());
        b
    }

    /// Decode from the fixed 32-byte wire form.
    pub fn decode(b: &[u8; RECORD_SIZE]) -> io::Result<Record> {
        let op = Op::from_u8(b[0])
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown trace op"))?;
        Ok(Record {
            op,
            align_log2: b[1],
            thread: u16::from_le_bytes([b[2], b[3]]),
            size: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            ptr: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            old_ptr: u64::from_le_bytes(b[24..32].try_into().unwrap()),
        })
    }
}

/// Streaming trace writer.
pub struct Writer<W: Write> {
    inner: W,
}

impl<W: Write> Writer<W> {
    /// Write the header and return a writer.
    pub fn new(mut inner: W) -> io::Result<Self> {
        inner.write_all(&MAGIC)?;
        inner.write_all(&FORMAT_VERSION.to_le_bytes())?;
        inner.write_all(&0u32.to_le_bytes())?;
        Ok(Writer { inner })
    }

    /// Append one record.
    pub fn write(&mut self, r: &Record) -> io::Result<()> {
        self.inner.write_all(&r.encode())
    }

    /// Flush and return the underlying writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.inner.flush()?;
        Ok(self.inner)
    }
}

/// Streaming trace reader.
pub struct Reader<R: Read> {
    inner: R,
}

impl<R: Read> Reader<R> {
    /// Validate the header and return a reader.
    pub fn new(mut inner: R) -> io::Result<Self> {
        let mut header = [0u8; 16];
        inner.read_exact(&mut header)?;
        if header[0..8] != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a .ratrace file",
            ));
        }
        let ver = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if ver != FORMAT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported trace version {ver}"),
            ));
        }
        Ok(Reader { inner })
    }

    /// Read the next record; `Ok(None)` at clean end-of-file.
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        let mut b = [0u8; RECORD_SIZE];
        match self.inner.read_exact(&mut b) {
            Ok(()) => Record::decode(&b).map(Some),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trip() {
        let records = [
            Record {
                op: Op::ThreadStart,
                align_log2: 0,
                thread: 0,
                size: 0,
                ptr: 0,
                old_ptr: 0,
            },
            Record {
                op: Op::Malloc,
                align_log2: 0,
                thread: 0,
                size: 48,
                ptr: 1,
                old_ptr: 0,
            },
            Record {
                op: Op::Zalloc,
                align_log2: 6,
                thread: 1,
                size: 4096,
                ptr: 2,
                old_ptr: 0,
            },
            Record {
                op: Op::Realloc,
                align_log2: 0,
                thread: 0,
                size: 96,
                ptr: 3,
                old_ptr: 1,
            },
            Record {
                op: Op::Free,
                align_log2: 0,
                thread: 1,
                size: 0,
                ptr: 0,
                old_ptr: 2,
            },
            Record {
                op: Op::ThreadEnd,
                align_log2: 0,
                thread: 1,
                size: 0,
                ptr: 0,
                old_ptr: 0,
            },
        ];

        let mut w = Writer::new(Vec::new()).unwrap();
        for r in &records {
            w.write(r).unwrap();
        }
        let bytes = w.finish().unwrap();
        assert_eq!(bytes.len(), 16 + records.len() * RECORD_SIZE);

        let mut rd = Reader::new(bytes.as_slice()).unwrap();
        let mut out = Vec::new();
        while let Some(r) = rd.next_record().unwrap() {
            out.push(r);
        }
        assert_eq!(out.as_slice(), records.as_slice());
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(Reader::new(&b"NOTATRACEFILE...."[..]).is_err());
    }
}
