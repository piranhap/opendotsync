use std::io::{Cursor, Read};
use std::path::Path;

use thiserror::Error;

use crate::collector::CollectedFile;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tar error: {0}")]
    Tar(String),
    #[error("file changed during snapshot: {0}")]
    FileChanged(String),
}

pub fn create_snapshot(
    _home_dir: &Path,
    files: &[CollectedFile],
) -> Result<Vec<u8>, SnapshotError> {
    let mut compressed = Vec::new();
    {
        let encoder = zstd::Encoder::new(&mut compressed, 3)?;
        let mut tar_builder = tar::Builder::new(encoder);

        for file in files {
            let relative = &file.relative_path;
            let absolute = &file.absolute_path;

            let data = read_with_retry(absolute, relative)?;

            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(file_mode(absolute));
            header.set_cksum();

            tar_builder
                .append_data(&mut header, relative, Cursor::new(&data))
                .map_err(|e| SnapshotError::Tar(e.to_string()))?;
        }

        let encoder = tar_builder
            .into_inner()
            .map_err(|e| SnapshotError::Tar(e.to_string()))?;
        encoder.finish()?;
    }

    Ok(compressed)
}

fn read_with_retry(path: &Path, relative: &str) -> Result<Vec<u8>, SnapshotError> {
    let pre_hash = crate::collector::compute_checksum(path)
        .map_err(|_| SnapshotError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to hash {relative}"),
        )))?;

    let data = std::fs::read(path)?;

    let post_hash = crate::collector::compute_checksum_bytes(&data);

    if pre_hash == post_hash {
        return Ok(data);
    }

    // File changed during read — retry up to 3 times
    for attempt in 1..=3 {
        std::thread::sleep(std::time::Duration::from_millis(100));

        let data = std::fs::read(path)?;
        let hash_before = crate::collector::compute_checksum(path)
            .map_err(|_| SnapshotError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to hash {relative} on retry {attempt}"),
            )))?;
        let hash_after = crate::collector::compute_checksum_bytes(&data);

        if hash_before == hash_after {
            return Ok(data);
        }
    }

    log::warn!("File {relative} changed during snapshot — using current state");
    Ok(std::fs::read(path)?)
}

fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o644)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o644
    }
}

pub fn extract_snapshot(data: &[u8]) -> Result<tempfile::TempDir, SnapshotError> {
    let tmp_dir = tempfile::tempdir()?;

    let decoder = zstd::Decoder::new(Cursor::new(data))?;
    let mut archive = tar::Archive::new(decoder);

    archive.unpack(tmp_dir.path())?;

    Ok(tmp_dir)
}

pub fn list_snapshot_contents(data: &[u8]) -> Result<Vec<String>, SnapshotError> {
    let decoder = zstd::Decoder::new(Cursor::new(data))?;
    let mut archive = tar::Archive::new(decoder);

    let mut paths = Vec::new();
    for entry in archive.entries().map_err(|e| SnapshotError::Tar(e.to_string()))? {
        let entry = entry.map_err(|e| SnapshotError::Tar(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| SnapshotError::Tar(e.to_string()))?;
        paths.push(path.to_string_lossy().to_string());
    }

    Ok(paths)
}

pub fn read_file_from_snapshot(
    data: &[u8],
    relative_path: &str,
) -> Result<Option<Vec<u8>>, SnapshotError> {
    let decoder = zstd::Decoder::new(Cursor::new(data))?;
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().map_err(|e| SnapshotError::Tar(e.to_string()))? {
        let mut entry = entry.map_err(|e| SnapshotError::Tar(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| SnapshotError::Tar(e.to_string()))?;

        if path.to_string_lossy() == relative_path {
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .map_err(|e| SnapshotError::Tar(e.to_string()))?;
            return Ok(Some(contents));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::CollectedFile;

    #[test]
    fn snapshot_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        std::fs::write(home.join(".bashrc"), "# my bashrc\nexport FOO=bar\n").unwrap();
        std::fs::write(home.join(".vimrc"), "set nocompatible\n").unwrap();

        let files = vec![
            CollectedFile {
                relative_path: ".bashrc".to_string(),
                absolute_path: home.join(".bashrc"),
                checksum: "sha256:unused".to_string(),
                size: 25,
                is_symlink: false,
            },
            CollectedFile {
                relative_path: ".vimrc".to_string(),
                absolute_path: home.join(".vimrc"),
                checksum: "sha256:unused".to_string(),
                size: 18,
                is_symlink: false,
            },
        ];

        let snapshot_data = create_snapshot(home, &files).unwrap();
        assert!(!snapshot_data.is_empty());

        let extracted = extract_snapshot(&snapshot_data).unwrap();
        let bashrc = std::fs::read_to_string(extracted.path().join(".bashrc")).unwrap();
        let vimrc = std::fs::read_to_string(extracted.path().join(".vimrc")).unwrap();

        assert_eq!(bashrc, "# my bashrc\nexport FOO=bar\n");
        assert_eq!(vimrc, "set nocompatible\n");
    }

    #[test]
    fn list_contents() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        std::fs::write(home.join("file1.txt"), "hello").unwrap();
        std::fs::write(home.join("file2.txt"), "world").unwrap();

        let files = vec![
            CollectedFile {
                relative_path: "file1.txt".to_string(),
                absolute_path: home.join("file1.txt"),
                checksum: "sha256:x".to_string(),
                size: 5,
                is_symlink: false,
            },
            CollectedFile {
                relative_path: "file2.txt".to_string(),
                absolute_path: home.join("file2.txt"),
                checksum: "sha256:x".to_string(),
                size: 5,
                is_symlink: false,
            },
        ];

        let data = create_snapshot(home, &files).unwrap();
        let contents = list_snapshot_contents(&data).unwrap();

        assert_eq!(contents.len(), 2);
        assert!(contents.contains(&"file1.txt".to_string()));
        assert!(contents.contains(&"file2.txt".to_string()));
    }

    #[test]
    fn read_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        std::fs::write(home.join("test.txt"), "content here").unwrap();

        let files = vec![CollectedFile {
            relative_path: "test.txt".to_string(),
            absolute_path: home.join("test.txt"),
            checksum: "sha256:x".to_string(),
            size: 12,
            is_symlink: false,
        }];

        let data = create_snapshot(home, &files).unwrap();

        let found = read_file_from_snapshot(&data, "test.txt").unwrap();
        assert_eq!(found.unwrap(), b"content here");

        let not_found = read_file_from_snapshot(&data, "missing.txt").unwrap();
        assert!(not_found.is_none());
    }
}
