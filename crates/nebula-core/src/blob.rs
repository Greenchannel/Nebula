//! Content-Addressed Storage (CAS) for images and voice messages.
//!
//! Media is *never* stored inside SQLite. The database only carries the
//! 64-character SHA-256 `blob_id`; the bytes live in `<data_dir>/blobs/<sha256>`.

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BlobStore {
    dir: PathBuf,
}

impl BlobStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let dir = root.as_ref().to_path_buf();
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating blob dir {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_for(&self, blob_id: &str) -> Result<PathBuf> {
        validate_blob_id(blob_id)?;
        Ok(self.dir.join(blob_id))
    }

    pub fn has(&self, blob_id: &str) -> bool {
        self.path_for(blob_id)
            .map(|p| p.is_file())
            .unwrap_or(false)
    }

    /// Hash arbitrary bytes and return the lowercase hex SHA-256.
    pub fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    /// Write bytes to the CAS atomically and return `(blob_id, size)`.
    pub fn put_bytes(&self, bytes: &[u8]) -> Result<(String, usize)> {
        let blob_id = Self::sha256_hex(bytes);
        let target = self.path_for(&blob_id)?;
        if !target.is_file() {
            let tmp = self.dir.join(format!(".{}.{}.tmp", blob_id, rand::random::<u32>()));
            fs::write(&tmp, bytes)
                .with_context(|| format!("writing temp blob {}", tmp.display()))?;
            // `rename` replaces atomically on Windows and POSIX alike only if the
            // target does not exist; tolerate a concurrent identical write.
            match fs::rename(&tmp, &target) {
                Ok(()) => {}
                Err(_) => {
                    let _ = fs::remove_file(&tmp);
                    if !target.is_file() {
                        // Retry with a direct write as a last resort.
                        fs::write(&target, bytes)?;
                    }
                }
            }
        }
        Ok((blob_id, bytes.len()))
    }

    /// Persist bytes received from a peer after verifying their hash matches
    /// the claimed `blob_id`.
    pub fn put_verified(&self, blob_id: &str, bytes: &[u8]) -> Result<()> {
        validate_blob_id(blob_id)?;
        let actual = Self::sha256_hex(bytes);
        if actual != blob_id {
            return Err(anyhow!(
                "blob hash mismatch: expected {blob_id}, got {actual}"
            ));
        }
        if self.has(blob_id) {
            return Ok(());
        }
        let target = self.dir.join(blob_id);
        let tmp = self
            .dir
            .join(format!(".{}.{}.tmp", blob_id, rand::random::<u32>()));
        fs::write(&tmp, bytes)?;
        match fs::rename(&tmp, &target) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = fs::remove_file(&tmp);
                if target.is_file() {
                    Ok(())
                } else {
                    Err(anyhow!("failed to commit blob {blob_id}"))
                }
            }
        }
    }

    pub fn read(&self, blob_id: &str) -> Result<Vec<u8>> {
        let path = self.path_for(blob_id)?;
        fs::read(&path).with_context(|| format!("reading blob {}", path.display()))
    }

    pub fn size(&self, blob_id: &str) -> Option<u64> {
        self.path_for(blob_id)
            .ok()
            .and_then(|p| fs::metadata(p).ok())
            .map(|m| m.len())
    }

    /// Verify an on-disk blob against its id.
    pub fn verify(&self, blob_id: &str) -> Result<bool> {
        let bytes = self.read(blob_id)?;
        Ok(Self::sha256_hex(&bytes) == blob_id)
    }
}

fn validate_blob_id(blob_id: &str) -> Result<()> {
    if blob_id.len() == 64 && blob_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(anyhow!("invalid blob id: {blob_id}"))
    }
}

/// Guess a MIME type from a file name / path.
pub fn guess_mime(name: &str) -> &'static str {
    let ext = name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "webm" => "audio/webm",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_stable_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path()).unwrap();
        let (id, len) = store.put_bytes(b"nebula").unwrap();
        assert_eq!(len, 6);
        assert_eq!(
            id,
            "16d74232d666243e3dd9711daaef2b7538f849efaa62cf19f91a97e82c420e34"
        );
        assert!(store.has(&id));
        assert!(store.verify(&id).unwrap());
        store.put_verified(&id, b"nebula").unwrap();
        assert!(store.put_verified(&id, b"tampered").is_err());
        assert!(store.put_verified("../../evil", b"x").is_err());
    }
}
