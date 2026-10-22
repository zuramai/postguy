//! Response body fixtures: deterministic bytes, compression (pure-Rust encoders only) and the streaming chunks. The zstd fixture is a hand-written single-frame
//! encoder using RAW blocks: valid zstd a decoder accepts, no C dependency.

use std::io::Write as _;

/// A deterministic body of `size` bytes: `0..9` cycling, so a test can verify byte positions.
pub fn pattern(size: usize) -> Vec<u8> {
    (0..size).map(|index| b'0' + ((index % 10) as u8)).collect()
}

/// gzip-compress `bytes` (flate2 with the miniz_oxide backend).
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// zlib/deflate-compress `bytes`.
pub fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// Brotli-compress `bytes`.
pub fn brotli(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let input = bytes;
    let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
    let _ = encoder.write_all(input);
    drop(encoder);
    out
}

/// Minimal valid zstd frame with RAW blocks (no compression): magic, frame header for a
/// single-segment frame whose content size is known, then chunked raw blocks.
/// Format reference: RFC 8878 §3.1.1. A decoder (ruzstd) accepts this exactly like any other
/// frame; the bytes on the wire are larger than the payload, which is fine for a fixture.
pub fn zstd_raw(bytes: &[u8]) -> Vec<u8> {
    const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
    let mut out = Vec::with_capacity(bytes.len() + 16);
    out.extend_from_slice(&MAGIC);
    // Frame header descriptor: FCS field size code 3 (8-byte content size), single segment.
    // Bits: 7-6 Frame_Content_Size_flag=11, bit5 Single_Segment=1, bit2 Content_Checksum=0.
    out.push(0b1110_0000);
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    // Raw blocks of at most 128 KiB; the last block carries the last-block bit.
    let mut chunks = bytes.chunks(128 * 1024).peekable();
    if chunks.peek().is_none() {
        // An empty raw block with last-block flag.
        out.extend_from_slice(&[0x01, 0x00, 0x00]);
        return out;
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        let size = chunk.len() as u32;
        let header = (size << 3) | u32::from(last);
        // Raw block: last bit in bit0 of the 3-byte Block_Header, block type 0, size in bits 3+.
        out.push((header & 0xFF) as u8);
        out.push(((header >> 8) & 0xFF) as u8);
        out.push(((header >> 16) & 0xFF) as u8);
        out.extend_from_slice(chunk);
    }
    out
}

/// The chunked streaming fixture: `chunks` pieces of `chunk_bytes` pattern bytes.
pub fn stream_chunks(chunks: usize, chunk_bytes: usize) -> Vec<Vec<u8>> {
    (0..chunks).map(|index| pattern(chunk_bytes).iter().map(|byte| byte.wrapping_add(index as u8)).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_is_position_addressable() {
        let bytes = pattern(23);
        assert_eq!(bytes.len(), 23);
        assert_eq!(bytes[0], b'0');
        assert_eq!(bytes[9], b'9');
        assert_eq!(bytes[10], b'0');
    }

    #[test]
    fn the_zstd_fixture_is_a_valid_single_frame() {
        let payload = pattern(1000);
        let frame = zstd_raw(&payload);
        assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD], "magic");
        assert_eq!(frame[4], 0b1110_0000);
        assert_eq!(&frame[5..13], &(1000u64).to_le_bytes());
        // Block header: size 1000 in bits 3+, last bit set.
        let header = u32::from_le_bytes([frame[13], frame[14], frame[15], 0]);
        assert_eq!(header >> 3, 1000);
        assert_eq!(header & 1, 1, "last block flag");
        assert_eq!(&frame[16..], payload.as_slice());
        // Empty input still produces a decodable empty frame.
        let empty = zstd_raw(&[]);
        assert_eq!(&empty[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        assert_eq!(&empty[5..13], &(0u64).to_le_bytes());
    }

    #[test]
    fn compressors_produce_recognisable_magic() {
        let payload = pattern(4096);
        assert_eq!(&gzip(&payload)[..2], &[0x1F, 0x8B]);
        let deflated = deflate(&payload);
        assert!(!deflated.is_empty());
        let brotli = brotli(&payload);
        assert!(!brotli.is_empty());
        assert!(brotli.len() < payload.len(), "pattern text compresses");
    }
}
