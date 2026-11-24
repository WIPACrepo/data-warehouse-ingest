//! SHA-512 checksum computation for file verification

use sha2::{Digest, Sha512};
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::error::{AppError, Result};

/// Compute SHA-512 checksum of a file in chunks
pub async fn compute_checksum(path: &Path, chunk_size: usize) -> Result<String> {
    let mut file = File::open(path)
        .await
        .map_err(|e| AppError::FileNotFound(format!("Failed to open file {:?}: {}", path, e)))?;

    let mut hasher = Sha512::new();
    let mut buffer = vec![0u8; chunk_size];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .await
            .map_err(|e| AppError::StorageError(format!("Failed to read file: {}", e)))?;

        if bytes_read == 0 {
            break;
        }

        hasher.update(&buffer[..bytes_read]);
    }

    let checksum = hasher.finalize();
    Ok(hex::encode(checksum))
}

/// Verify that a file matches an expected checksum
pub async fn verify_checksum(path: &Path, expected: &str, chunk_size: usize) -> Result<bool> {
    let actual = compute_checksum(path, chunk_size).await?;
    Ok(actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_compute_checksum() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let test_data = b"Hello, World!";
        temp_file.write_all(test_data).unwrap();

        let checksum = compute_checksum(temp_file.path(), 1024).await.unwrap();
        assert!(!checksum.is_empty());
        assert_eq!(checksum.len(), 128); // SHA-512 produces 64 bytes = 128 hex chars
    }

    #[tokio::test]
    async fn test_verify_checksum() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let test_data = b"Hello, World!";
        temp_file.write_all(test_data).unwrap();

        let checksum = compute_checksum(temp_file.path(), 1024).await.unwrap();
        let verified = verify_checksum(temp_file.path(), &checksum, 1024)
            .await
            .unwrap();
        assert!(verified);

        let wrong_verified = verify_checksum(temp_file.path(), "wrong_checksum", 1024)
            .await
            .unwrap();
        assert!(!wrong_verified);
    }
}
