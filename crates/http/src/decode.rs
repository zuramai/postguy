//! Response decoding: gzip, deflate (zlib, with a raw-deflate
//! fallback), brotli and zstd (pure-Rust `ruzstd` decoder). Caps are mandatory: a decoded size
//! cap and a ratio cap, so a compression bomb errors instead of exhausting memory (AGENTS.md 4
//! names this explicitly). Chained encodings (`Content-Encoding: br, gzip`) decode in reverse.

use std::io::Read;

use crate::error::{HttpError, Result};

/// Decoding limits. Callers wire them from the request settings; the defaults are the v1
/// product defaults (50 MB response cap, 200x ratio keeps ordinary text well inside).
#[derive(Debug, Clone, Copy)]
pub struct DecodeCaps {
    pub max_decoded_bytes: usize,
    pub max_ratio: u32,
}

impl Default for DecodeCaps {
    fn default() -> Self {
        DecodeCaps { max_decoded_bytes: 50 << 20, max_ratio: 200 }
    }
}

/// A decoded body with the numbers the viewer shows on hover (headers vs body, encoded vs
/// decoded).
#[derive(Debug)]
pub struct DecodedBody {
    pub bytes: Vec<u8>,
    /// `Content-Encoding` as received (empty = none).
    pub encoding: String,
    pub encoded_bytes: usize,
    pub decoded_bytes: usize,
}

/// Parse a `Content-Encoding` value into the encodings to undo, in REVERSE application order.
pub fn encodings(header: &str) -> Vec<String> {
    let mut list: Vec<String> = header
        .split(',')
        .map(|encoding| encoding.trim().to_ascii_lowercase())
        .filter(|encoding| !encoding.is_empty() && encoding != "identity")
        .collect();
    list.reverse();
    list
}

/// Decode `bytes` by the `Content-Encoding` header, enforcing the caps on the FINAL decoded
/// size and the overall ratio (encoded vs decoded).
pub fn decode(bytes: &[u8], content_encoding: &str, caps: DecodeCaps) -> Result<DecodedBody> {
    let chain = encodings(content_encoding);
    // Display label keeps the header's order (applied order), while `chain` is reversed for
    // decoding.
    let mut display = chain.clone();
    display.reverse();
    let display_label = display.join(", ");
    let encoded_bytes = bytes.len();
    let mut current: Vec<u8> = bytes.to_vec();
    for encoding in &chain {
        current = decode_one(&current, encoding, caps)?;
        // Check the intermediate size too: a chain of bombs still hits the cap early.
        if current.len() > caps.max_decoded_bytes {
            return Err(HttpError::BodyTooLarge { cap: caps.max_decoded_bytes });
        }
    }
    let decoded_bytes = current.len();
    if decoded_bytes > caps.max_decoded_bytes {
        return Err(HttpError::BodyTooLarge { cap: caps.max_decoded_bytes });
    }
    // Ratio bomb: tiny encoded payloads that expand absurdly. Guarded for tiny bodies where
    // ratios are meaningless (64 KiB absolute headroom).
    if encoded_bytes > 0 {
        let ratio_bound = encoded_bytes.saturating_mul(caps.max_ratio as usize).saturating_add(64 * 1024);
        if decoded_bytes > ratio_bound {
            return Err(HttpError::Protocol {
                reason: format!("compression ratio {decoded_bytes}/{encoded_bytes} exceeds the {}-to-1 cap", caps.max_ratio),
            });
        }
    }
    Ok(DecodedBody { bytes: current, encoding: display_label, encoded_bytes, decoded_bytes })
}

fn decode_one(bytes: &[u8], encoding: &str, caps: DecodeCaps) -> Result<Vec<u8>> {
    match encoding {
        "gzip" | "x-gzip" => read_capped(flate2::read::GzDecoder::new(bytes), caps),
        "deflate" => {
            // HTTP `deflate` is zlib; some servers send raw deflate. Try zlib, then raw when
            // nothing was produced (never a silent half-decode).
            let mut zlib_out = Vec::new();
            let zlib = flate2::read::ZlibDecoder::new(bytes);
            match read_capped_into(zlib, &mut zlib_out, caps) {
                Ok(()) => Ok(zlib_out),
                Err(err) if zlib_out.is_empty() && matches!(err, HttpError::Protocol { .. }) => {
                    read_capped(flate2::read::DeflateDecoder::new(bytes), caps)
                }
                Err(err) => Err(err),
            }
        }
        "br" => read_capped(brotli::Decompressor::new(bytes, 8 * 1024), caps),
        "zstd" => read_capped(
            ruzstd::decoding::StreamingDecoder::new(bytes).map_err(|err| HttpError::Protocol { reason: format!("zstd frame: {err}") })?,
            caps,
        ),
        other => Err(HttpError::Protocol { reason: format!("unsupported content-encoding `{other}`") }),
    }
}

fn read_capped(reader: impl Read, caps: DecodeCaps) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    read_capped_into(reader, &mut out, caps)?;
    Ok(out)
}

/// Copy a decoder to `out`, refusing the moment the decoded size passes the cap. The initial
/// allocation is bounded by the cap as well (never allocate by the header's claims).
fn read_capped_into(reader: impl Read, out: &mut Vec<u8>, caps: DecodeCaps) -> Result<()> {
    out.reserve((64 * 1024).min(caps.max_decoded_bytes));
    let mut chunk = [0u8; 16 * 1024];
    let mut decoder = reader;
    loop {
        match decoder.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(read) => {
                if out.len().saturating_add(read) > caps.max_decoded_bytes {
                    return Err(HttpError::BodyTooLarge { cap: caps.max_decoded_bytes });
                }
                out.extend_from_slice(&chunk[..read]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                return Err(HttpError::Protocol { reason: format!("decoding body: {err}") });
            }
        }
    }
}

/// Header bytes as received (status line and header names/values) for the size breakdown.
/// The exact status-line spelling (HTTP/2 has none) is approximated by the caller's version.
pub fn header_bytes(status: u16, headers: &[(String, String)]) -> usize {
    // "HTTP/1.1 200 OK\r\n" ≈ 15 bytes before the reason; a stable approximation keeps the
    // number honest without inventing a reason phrase the server didn't send.
    let mut total = 17usize;
    for (name, value) in headers {
        total = total.saturating_add(name.len()).saturating_add(value.len()).saturating_add(4); // ": " + CRLF
    }
    let _ = status;
    total.saturating_add(2) // final CRLF
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn pattern(size: usize) -> Vec<u8> {
        (0..size).map(|index| b'0' + ((index % 10) as u8)).collect()
    }

    #[test]
    fn gzip_deflate_br_and_zstd_decode_to_the_same_bytes() {
        let payload = pattern(4096);
        let caps = DecodeCaps::default();
        // gzip
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&payload).expect("write");
        let gz = encoder.finish().expect("finish");
        assert_eq!(decode(&gz, "gzip", caps).expect("gzip").bytes, payload);
        // deflate (zlib)
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&payload).expect("write");
        let zlib = encoder.finish().expect("finish");
        assert_eq!(decode(&zlib, "deflate", caps).expect("deflate").bytes, payload);
        // brotli
        let mut out = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
            encoder.write_all(&payload).expect("write");
        }
        assert_eq!(decode(&out, "br", caps).expect("brotli").bytes, payload);
        // zstd raw-block fixture (the testkit's tiny encoder)
        let mut frame = vec![0x28, 0xB5, 0x2F, 0xFD, 0b1110_0000];
        frame.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        let header = ((payload.len() as u32) << 3) | 1;
        frame.extend_from_slice(&header.to_le_bytes()[..3]);
        frame.extend_from_slice(&payload);
        assert_eq!(decode(&frame, "zstd", caps).expect("zstd").bytes, payload);
    }

    #[test]
    fn chained_encodings_apply_in_reverse() {
        // Wire order: gzip then br; decode applies br first, then gzip.
        let payload = pattern(2048);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&payload).expect("write");
        let gz = gz.finish().expect("finish");
        let mut br = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            encoder.write_all(&gz).expect("write");
        }
        let decoded = decode(&br, "gzip, br", DecodeCaps::default()).expect("decode chain");
        assert_eq!(decoded.bytes, payload);
        assert_eq!(decoded.encoding, "gzip, br", "header order preserved for display");
    }

    #[test]
    fn a_compression_bomb_hits_the_ratio_cap() {
        // 8 MiB of zeros compress to a few KiB; with a 10:1 ratio cap this must refuse.
        let bomb = vec![0u8; 8 << 20];
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&bomb).expect("write");
        let gz = gz.finish().expect("finish");
        let caps = DecodeCaps { max_decoded_bytes: 64 << 20, max_ratio: 10 };
        let err = decode(&gz, "gzip", caps).expect_err("ratio cap");
        assert!(matches!(err, HttpError::Protocol { .. }), "{err}");
        assert!(err.to_string().contains("ratio"), "{err}");
    }

    #[test]
    fn a_decoded_size_cap_stops_mid_stream() {
        let bomb = vec![7u8; 4 << 20];
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&bomb).expect("write");
        let gz = gz.finish().expect("finish");
        let caps = DecodeCaps { max_decoded_bytes: 128 * 1024, max_ratio: 10_000 };
        let err = decode(&gz, "gzip", caps).expect_err("size cap");
        assert!(matches!(err, HttpError::BodyTooLarge { cap: 131_072 }), "{err}");
    }

    #[test]
    fn unknown_and_absent_encodings() {
        assert_eq!(encodings(""), Vec::<String>::new());
        assert_eq!(encodings("identity"), Vec::<String>::new());
        let err = decode(b"x", "snappy", DecodeCaps::default()).expect_err("unknown");
        assert!(err.to_string().contains("snappy"), "{err}");
        // No encodings: bytes pass through unchanged and the numbers agree.
        let decoded = decode(b"hello", "", DecodeCaps::default()).expect("plain");
        assert_eq!(decoded.bytes, b"hello");
        assert_eq!(decoded.encoded_bytes, decoded.decoded_bytes);
    }

    #[test]
    fn raw_deflate_fallback_decodes_data_without_zlib_headers() {
        let payload = pattern(1024);
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&payload).expect("write");
        let raw = encoder.finish().expect("finish");
        let decoded = decode(&raw, "deflate", DecodeCaps::default()).expect("raw deflate fallback");
        assert_eq!(decoded.bytes, payload);
    }

    #[test]
    fn header_size_breakdown_is_approximate_but_monotonic() {
        let small = header_bytes(200, &[("a".into(), "b".into())]);
        let larger = header_bytes(200, &[("a".into(), "b".into()), ("content-type".into(), "application/json".into())]);
        assert!(larger > small);
        assert!(small > 15);
    }
}
