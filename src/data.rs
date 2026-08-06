//! Reader for the embedded data containers.
//!
//! Both `data/en_us.dat` (language) and `data/cmu_us_kal.dat` (voice) use one
//! trivial container so the crate needs no serialisation dependency and no
//! `build.rs`:
//!
//! ```text
//! magic   "FLRSDAT\x01"
//! section  u8 name_len | name | u32 payload_len | payload      (repeated)
//! ```
//!
//! All integers are little-endian. Payloads are *not* aligned, so multi-byte
//! values are read with `from_le_bytes` rather than transmuted. This keeps the
//! reader safe and endian-independent, which matters more than the handful of
//! nanoseconds it costs. Frames are decoded on demand during synthesis, so
//! nothing here is copied at load time.
//!
//! `tools/gen_data.py` writes these files; its docstring is the other half of
//! this format's documentation.

use std::fmt;

const MAGIC: &[u8; 8] = b"FLRSDAT\x01";

/// Data file failed to parse. Since the files are embedded at compile time,
/// this only fires when the container and the reader are out of step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataError(pub &'static str);

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed flite-rs data file: {}", self.0)
    }
}

impl std::error::Error for DataError {}

type Result<T> = std::result::Result<T, DataError>;

/// The named sections of one container, borrowed from the embedded bytes.
pub struct Container<'a> {
    sections: Vec<(&'a str, &'a [u8])>,
}

impl<'a> Container<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Container<'a>> {
        if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
            return Err(DataError("bad magic"));
        }
        let mut sections = Vec::new();
        let mut r = Reader::new(&bytes[MAGIC.len()..]);
        while !r.is_empty() {
            let name_len = r.u8()? as usize;
            let name = r.str(name_len)?;
            let len = r.u32()? as usize;
            sections.push((name, r.bytes(len)?));
        }
        Ok(Container { sections })
    }

    pub fn section(&self, name: &str) -> Result<&'a [u8]> {
        self.optional_section(name)
            .ok_or(DataError("missing section"))
    }

    /// A section that a file may legitimately not carry, such as the decoded
    /// residual lengths of a voice whose residual is not compressed.
    pub fn optional_section(&self, name: &str) -> Option<&'a [u8]> {
        self.sections
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| *b)
    }
}

/// Sequential cursor over a byte slice.
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(DataError("length overflow"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(DataError("truncated section"))?;
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&mut self) -> Result<f32> {
        let b = self.bytes(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn str(&mut self, len: usize) -> Result<&'a str> {
        std::str::from_utf8(self.bytes(len)?).map_err(|_| DataError("invalid utf-8"))
    }

    /// A length-prefixed string (one byte of length).
    pub fn short_str(&mut self) -> Result<&'a str> {
        let len = self.u8()? as usize;
        self.str(len)
    }

    /// A `u32` count followed by that many length-prefixed strings.
    pub fn string_table(&mut self) -> Result<Vec<&'a str>> {
        let n = self.u32()? as usize;
        (0..n).map(|_| self.short_str()).collect()
    }
}

/// Read the `i`-th little-endian `u16` of a packed array.
#[inline]
pub fn u16_at(bytes: &[u8], i: usize) -> u16 {
    let o = i * 2;
    u16::from_le_bytes([bytes[o], bytes[o + 1]])
}

/// Read the `i`-th little-endian `u32` of a packed array.
#[inline]
pub fn u32_at(bytes: &[u8], i: usize) -> u32 {
    let o = i * 4;
    u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
}
