//! Builds the compressed body for the byte-cap test.
//!
//! The point of that test is that the cap counts *decompressed* bytes: a small wire body must
//! trip it, and the wire `Content-Length` pre-check must not be what trips it. A run of one
//! repeated byte compresses at an enormous ratio, which is exactly the shape of the attack the
//! cap exists to stop.

use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

/// How many bytes `gzip_repeated_byte(_, repeats)` expands to.
pub fn decompressed_len(repeats: usize) -> usize {
    repeats
}

/// A gzip stream that decompresses to `repeats` copies of `byte`.
pub fn gzip_repeated_byte(byte: u8, repeats: usize) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    // Feed in chunks so the source run never has to exist in memory all at once — the whole
    // point is that this side stays small while the decompressed side does not.
    const CHUNK: usize = 64 * 1024;
    let chunk = vec![byte; CHUNK.min(repeats.max(1))];
    let mut left = repeats;
    while left > 0 {
        let n = left.min(chunk.len());
        enc.write_all(&chunk[..n])
            .expect("writing to an in-memory encoder cannot fail");
        left -= n;
    }
    enc.finish()
        .expect("finishing an in-memory encoder cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    #[test]
    fn round_trips_and_compresses_hard() {
        let repeats = 1024 * 1024;
        let wire = gzip_repeated_byte(b'A', repeats);
        assert!(
            wire.len() * 100 < repeats,
            "expected better than 100:1, got {} -> {repeats}",
            wire.len()
        );

        let mut out = Vec::new();
        GzDecoder::new(&wire[..])
            .read_to_end(&mut out)
            .expect("valid gzip");
        assert_eq!(out.len(), decompressed_len(repeats));
        assert!(out.iter().all(|b| *b == b'A'));
    }
}
