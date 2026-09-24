//! The content hash: what a file is called when its path is not to
//! be trusted.
//!
//! BLAKE3 over the first 64 KB and the file's size, as §72 decided:
//! a raw's header, its EXIF and the start of its embedded preview
//! are in the first 64 KB, and two frames from the same body a
//! fraction of a second apart differ there in the timestamp, the
//! frame counter and the preview's pixels; the size guards the rest.
//! Whole-file hashing is what makes an import in the other tools
//! take an hour, and a library index cannot afford it on every
//! launch. BLAKE3 rather than SHA-256 because it is several times
//! faster on 64 KB and the hash is a key, not a signature.

use std::io::Read;
use std::path::Path;

/// How much of the file's head goes into the hash.
pub const HEAD: usize = 64 * 1024;

/// The content hash of a file on disk, as lowercase hex.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = vec![0u8; HEAD];
    let mut read = 0;
    while read < HEAD {
        match file.read(&mut head[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(hash_bytes(&head[..read], size))
}

/// The hash of a file whose head and size are already in hand.
pub fn hash_bytes(head: &[u8], size: u64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&head[..head.len().min(HEAD)]);
    hasher.update(&size.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_the_head_and_the_size() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-hash-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let body: Vec<u8> = (0..HEAD + 1000).map(|i| (i % 251) as u8).collect();
        let a = dir.join("a.bin");
        std::fs::write(&a, &body).unwrap();
        let hash_a = hash_file(&a).unwrap();
        assert_eq!(hash_a.len(), 64);
        assert_eq!(hash_a, hash_bytes(&body[..HEAD], body.len() as u64));
        // A byte past the head does not change it; the size does.
        let mut tail_changed = body.clone();
        tail_changed[HEAD + 10] ^= 0xff;
        let b = dir.join("b.bin");
        std::fs::write(&b, &tail_changed).unwrap();
        assert_eq!(hash_file(&b).unwrap(), hash_a);
        let mut longer = body.clone();
        longer.push(0);
        std::fs::write(&b, &longer).unwrap();
        assert_ne!(hash_file(&b).unwrap(), hash_a);
        // A byte in the head does.
        let mut head_changed = body.clone();
        head_changed[100] ^= 0xff;
        std::fs::write(&b, &head_changed).unwrap();
        assert_ne!(hash_file(&b).unwrap(), hash_a);
        // A file shorter than the head hashes what there is.
        std::fs::write(&b, b"short").unwrap();
        assert_eq!(hash_file(&b).unwrap(), hash_bytes(b"short", 5));
        assert_eq!(hash_file(&b).unwrap(), hash_bytes(b"short", 5));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
