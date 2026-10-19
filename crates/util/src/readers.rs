//! Size-capped file readers: read whole files but refuse to slurp anything beyond the cap.
//! `postguy-util` never loads an oversized file into memory; the size is known before all of
//! it is read.

use std::path::Path;

use crate::error::{Error, Result};

/// Read a whole file into `Vec<u8>`, expecting at most `cap` bytes.
///
/// The cap is checked twice: once from the file metadata (cheap, before any read), and again
/// while reading, so a file that grows between the two checks (or whose metadata lies) is cut
/// off by the streaming read, not by trust in the metadata.
pub fn read_file_capped(path: &Path, cap: usize) -> Result<Vec<u8>> {
    let operation = format!("reading {}", path.display());
    let metadata = std::fs::metadata(path).map_err(|source| Error::Io { operation: operation.clone(), source })?;
    if !metadata.is_file() {
        return Err(Error::Io { operation, source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file") });
    }
    let len = metadata.len();
    if len > cap as u64 {
        return Err(Error::TooLarge { what: operation, size: len as usize, cap });
    }

    let mut bytes = Vec::with_capacity(len as usize);
    let mut file = std::fs::File::open(path).map_err(|source| Error::Io { operation: operation.clone(), source })?;
    limit_read(&mut file, &mut bytes, cap as u64, operation)?;
    Ok(bytes)
}

/// Read a whole file as UTF-8 text, at most `cap` bytes.
pub fn read_string_capped(path: &Path, cap: usize) -> Result<String> {
    let bytes = read_file_capped(path, cap)?;
    String::from_utf8(bytes).map_err(|_| Error::Encoding { what: format!("reading {}", path.display()) })
}

/// Append from `reader` into `out` while bounding it at `cap` bytes total. Oversize is caught
/// during the read: no trusted metadata involvement.
fn limit_read(reader: &mut impl std::io::Read, out: &mut Vec<u8>, cap: u64, what: String) -> Result<()> {
    let mut chunk = [0u8; 16 * 1024];
    let mut read_total: u64 = out.len() as u64;
    loop {
        let n = reader.read(&mut chunk).map_err(|source| Error::Io { operation: format!("reading {what}"), source })?;
        if n == 0 {
            return Ok(());
        }
        read_total += u64::try_from(n).map_err(|_| Error::Unusable { what: "read length overflowed u64".to_string() })?;
        if read_total > cap {
            out.clear();
            let size = usize::try_from(read_total).unwrap_or(usize::MAX);
            return Err(Error::TooLarge { what, size, cap: cap as usize });
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::path::PathBuf;

    fn unique_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("postguy-util-tests-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_a_small_file_whole() {
        let dir = unique_dir("small");
        let path = dir.join("small.txt");
        std::fs::write(&path, b"hello postguy").unwrap();
        let bytes = read_file_capped(&path, 100).unwrap();
        assert_eq!(bytes, b"hello postguy");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_to_read_a_directory() {
        let dir = unique_dir("dir");
        let err = read_file_capped(&dir, 100).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_oversize_from_metadata_before_reading() {
        let dir = unique_dir("over");
        let path = dir.join("big.bin");
        // 2 MiB file, cap 1 KiB: TooLarge without any content in memory.
        let big = vec![0u8; 2 * 1024 * 1024];
        std::fs::write(&path, &big).unwrap();
        match read_file_capped(&path, 1024) {
            Err(Error::TooLarge { size, cap, .. }) => {
                assert_eq!(size, 2 * 1024 * 1024);
                assert_eq!(cap, 1024);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn streaming_guard_does_not_trust_knowledge_of_length() {
        // A named pipe can pass a metadata check with a short length and then stream forever;
        // this covers the in-read cap directly with a 3-byte reader against a 2-byte cap.
        let mut reader = std::io::Cursor::new(vec![7u8; 3]);
        let mut out = Vec::new();
        let err = limit_read(&mut reader, &mut out, 2, "stream".to_string()).unwrap_err();
        assert!(matches!(err, Error::TooLarge { cap: 2, .. }), "{err}");
        assert!(out.is_empty());
    }

    #[test]
    fn string_reader_rejects_invalid_utf8() {
        let dir = unique_dir("utf8");
        let path = dir.join("bin.txt");
        std::fs::write(&path, [0xFF, 0xFE, 0xFD]).unwrap();
        let err = read_string_capped(&path, 100).unwrap_err();
        assert!(matches!(err, Error::Encoding { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }
}
