//! A minimal, hand-written, spec-compliant single-block DEFLATE encoder for exactly one job:
//! "repeat this byte many times" — used to build a gzip response body whose decompressed size is
//! orders of magnitude larger than its wire size, to prove `http::body::read_capped` caps on the
//! *decompressed* count, not `Content-Length`.
//!
//! This crate has no compression-encoding dependency (`reqwest`'s `gzip` feature only gets it a
//! *decoder*, via `tower-http`, and this chunk cannot add a dev-dependency — see the chunk
//! report), so this hand-rolls RFC 1951's fixed-Huffman block encoding for the one pattern it
//! needs: one literal byte, then repeated maximum-length (258-byte), minimum-distance (1-byte)
//! back-references. That yields a compression ratio around 160:1 — nowhere near a real
//! compressor's ratio on this input, but easily enough to turn a ~100 KB wire body into tens of
//! megabytes decompressed, at loopback speed, inside one test. The bit-packing was verified
//! independently against Python's `zlib`/`gzip` modules before being ported here.

/// Bits are pushed LSB-first into each output byte, matching DEFLATE's own bit-packing order.
struct BitWriter {
    bytes: Vec<u8>,
    cur: u8,
    nbits: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    fn push_bit(&mut self, bit: u8) {
        self.cur |= (bit & 1) << self.nbits;
        self.nbits += 1;
        if self.nbits == 8 {
            self.bytes.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    /// Ordinary multi-bit DEFLATE fields (block headers, extra bits) are packed LSB-of-value
    /// first.
    fn push_lsb(&mut self, value: u32, nbits: u32) {
        for i in 0..nbits {
            self.push_bit(((value >> i) & 1) as u8);
        }
    }

    /// Huffman codes are the one exception (RFC 1951 §3.1.1): packed MSB-of-code first.
    fn push_huffman(&mut self, code: u32, nbits: u32) {
        for i in (0..nbits).rev() {
            self.push_bit(((code >> i) & 1) as u8);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.bytes.push(self.cur);
        }
        self.bytes
    }
}

/// The fixed-Huffman code for literal byte `v` (RFC 1951 §3.2.6): 0..=143 get an 8-bit code,
/// 144..=255 get a 9-bit code.
fn fixed_literal_code(v: u8) -> (u32, u32) {
    let v = u32::from(v);
    if v <= 143 {
        (0x30 + v, 8)
    } else {
        (0x190 + (v - 144), 9)
    }
}

/// A raw DEFLATE stream (no zlib/gzip wrapper): one literal `byte`, then `repeats` maximum-length
/// back-references, replicating it. Decompresses to exactly `1 + repeats * 258` bytes, all equal
/// to `byte`.
fn deflate_repeated_byte(byte: u8, repeats: usize) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.push_lsb(1, 1); // BFINAL = 1 (final, and only, block)
    w.push_lsb(1, 2); // BTYPE = 01 (fixed Huffman)

    let (code, nbits) = fixed_literal_code(byte);
    w.push_huffman(code, nbits);

    for _ in 0..repeats {
        // Length code 285 = length 258, 0 extra bits (fixed Huffman: codes 280..=287 are 8 bits,
        // value 280 + i).
        w.push_huffman(0xC0 + 5, 8);
        // Distance code 0 = distance 1, 0 extra bits (fixed Huffman: all 30 distance codes are a
        // plain 5-bit value).
        w.push_huffman(0, 5);
    }
    w.push_huffman(0, 7); // end-of-block (symbol 256; fixed Huffman: codes 256..=279 are 7 bits)
    w.finish()
}

/// IEEE 802.3 CRC-32 (the gzip trailer's checksum) of `count` repetitions of `byte`, computed
/// without materialising the (potentially huge) decompressed buffer.
fn crc32_of_repeated_byte(byte: u8, count: usize) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }

    let mut crc = 0xFFFF_FFFFu32;
    for _ in 0..count {
        let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = table[idx] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// The number of bytes a gzip body built by [`gzip_repeated_byte`] decompresses to.
pub fn decompressed_len(repeats: usize) -> usize {
    1 + repeats * 258
}

/// A complete gzip container (magic, header, DEFLATE payload, CRC32 + size trailer) whose
/// decompressed content is `byte` repeated [`decompressed_len`]`(repeats)` times.
pub fn gzip_repeated_byte(byte: u8, repeats: usize) -> Vec<u8> {
    let deflate = deflate_repeated_byte(byte, repeats);
    let len = decompressed_len(repeats) as u32;
    let crc = crc32_of_repeated_byte(byte, decompressed_len(repeats));

    let mut out = Vec::with_capacity(10 + deflate.len() + 8);
    // Magic (1f 8b), CM=deflate (08), FLG=0, MTIME=0, XFL=0, OS=unknown (ff).
    out.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0, 0xff]);
    out.extend_from_slice(&deflate);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out
}
