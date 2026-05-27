use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use walkdir::WalkDir;

use crate::paths::PathsList;

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error("IO error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("snapshot exceeds size limit: {0} bytes (limit: {1} bytes)")]
    SizeLimitExceeded(u64, u64),
}

#[derive(Debug, Clone)]
pub struct CollectedFile {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub checksum: String,
    pub size: u64,
    pub is_symlink: bool,
}

#[derive(Debug)]
pub struct SizeWarning {
    pub path: String,
    pub size: u64,
    pub message: String,
}

const SINGLE_FILE_WARN: u64 = 10 * 1024 * 1024; // 10 MB
const TOTAL_WARN: u64 = 100 * 1024 * 1024; // 100 MB
#[allow(dead_code)]
const TOTAL_HARD_LIMIT: u64 = 500 * 1024 * 1024; // 500 MB

pub fn collect_files(
    home_dir: &Path,
    paths_list: &PathsList,
) -> Result<Vec<CollectedFile>, CollectorError> {
    let mut files = Vec::new();

    for entry in &paths_list.include {
        let entry_path = &entry.path;
        let absolute = home_dir.join(entry_path);

        if entry_path.ends_with('/') {
            collect_directory(home_dir, &absolute, paths_list, &mut files)?;
        } else {
            collect_single_file(home_dir, &absolute, paths_list, &mut files)?;
        }
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    files.dedup_by(|a, b| a.relative_path == b.relative_path);

    Ok(files)
}

fn collect_directory(
    home_dir: &Path,
    dir_path: &Path,
    paths_list: &PathsList,
    files: &mut Vec<CollectedFile>,
) -> Result<(), CollectorError> {
    if !dir_path.exists() {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(dir_path).map_err(|e| CollectorError::Io {
        path: dir_path.to_path_buf(),
        source: e,
    })?;

    // Skip directory symlinks (Section 10.4)
    if metadata.is_symlink() {
        log::warn!(
            "Skipping directory symlink: {}",
            dir_path.display()
        );
        return Ok(());
    }

    for walker_entry in WalkDir::new(dir_path).follow_links(false) {
        let walker_entry = match walker_entry {
            Ok(e) => e,
            Err(err) => {
                log::warn!("Skipping inaccessible path: {}", err);
                continue;
            }
        };

        let file_metadata = walker_entry.metadata().map_err(|e| CollectorError::Io {
            path: walker_entry.path().to_path_buf(),
            source: e.into(),
        })?;

        if file_metadata.is_dir() {
            continue;
        }

        let absolute = walker_entry.path().to_path_buf();

        // Handle symlinks
        let symlink_meta =
            std::fs::symlink_metadata(&absolute).map_err(|e| CollectorError::Io {
                path: absolute.clone(),
                source: e,
            })?;

        if symlink_meta.is_symlink() {
            // Check if it's a broken symlink
            if !absolute.exists() {
                continue;
            }
            // Follow file symlinks — get the real metadata
            let real_meta = std::fs::metadata(&absolute).map_err(|e| CollectorError::Io {
                path: absolute.clone(),
                source: e,
            })?;
            if real_meta.is_dir() {
                log::warn!(
                    "Skipping directory symlink: {}",
                    absolute.display()
                );
                continue;
            }
        }

        let relative = absolute
            .strip_prefix(home_dir)
            .unwrap_or(&absolute)
            .to_string_lossy()
            .to_string();

        if crate::paths::is_excluded(paths_list, &relative) {
            continue;
        }

        let checksum = compute_checksum(&absolute)?;

        files.push(CollectedFile {
            relative_path: relative,
            absolute_path: absolute,
            checksum,
            size: file_metadata.len(),
            is_symlink: symlink_meta.is_symlink(),
        });
    }

    Ok(())
}

fn collect_single_file(
    home_dir: &Path,
    file_path: &Path,
    paths_list: &PathsList,
    files: &mut Vec<CollectedFile>,
) -> Result<(), CollectorError> {
    if !file_path.exists() {
        return Ok(());
    }

    let symlink_meta =
        std::fs::symlink_metadata(file_path).map_err(|e| CollectorError::Io {
            path: file_path.to_path_buf(),
            source: e,
        })?;

    // Skip broken symlinks
    if symlink_meta.is_symlink() && !file_path.exists() {
        return Ok(());
    }

    // Skip directory symlinks
    if symlink_meta.is_symlink() {
        let real_meta = std::fs::metadata(file_path).map_err(|e| CollectorError::Io {
            path: file_path.to_path_buf(),
            source: e,
        })?;
        if real_meta.is_dir() {
            log::warn!(
                "Skipping directory symlink: {}",
                file_path.display()
            );
            return Ok(());
        }
    }

    let relative = file_path
        .strip_prefix(home_dir)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    if crate::paths::is_excluded(paths_list, &relative) {
        return Ok(());
    }

    let metadata = std::fs::metadata(file_path).map_err(|e| CollectorError::Io {
        path: file_path.to_path_buf(),
        source: e,
    })?;

    let checksum = compute_checksum(file_path)?;

    files.push(CollectedFile {
        relative_path: relative,
        absolute_path: file_path.to_path_buf(),
        checksum,
        size: metadata.len(),
        is_symlink: symlink_meta.is_symlink(),
    });

    Ok(())
}

pub fn compute_checksum(path: &Path) -> Result<String, CollectorError> {
    let data = std::fs::read(path).map_err(|e| CollectorError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();

    let hex: String = result.iter().map(|b| format!("{:02x}", b)).collect();
    Ok(format!("sha256:{hex}"))
}

pub fn compute_checksum_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let hex: String = result.iter().map(|b| format!("{:02x}", b)).collect();
    format!("sha256:{hex}")
}

pub fn check_size_limits(files: &[CollectedFile]) -> Vec<SizeWarning> {
    let mut warnings = Vec::new();

    for file in files {
        if file.size > SINGLE_FILE_WARN {
            warnings.push(SizeWarning {
                path: file.relative_path.clone(),
                size: file.size,
                message: format!(
                    "{} is {} MB (warning threshold: {} MB)",
                    file.relative_path,
                    file.size / (1024 * 1024),
                    SINGLE_FILE_WARN / (1024 * 1024)
                ),
            });
        }
    }

    let total: u64 = files.iter().map(|f| f.size).sum();
    if total > TOTAL_WARN {
        warnings.push(SizeWarning {
            path: "(total)".to_string(),
            size: total,
            message: format!(
                "Total snapshot size is {} MB (warning threshold: {} MB)",
                total / (1024 * 1024),
                TOTAL_WARN / (1024 * 1024)
            ),
        });
    }

    warnings
}

pub fn total_size(files: &[CollectedFile]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{MergeType, PathEntry};

    #[test]
    fn collect_files_basic() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        std::fs::write(home.join(".bashrc"), "# bashrc").unwrap();
        std::fs::write(home.join(".vimrc"), "\" vimrc").unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/config"), "Host *").unwrap();
        std::fs::write(home.join(".ssh/id_rsa"), "secret").unwrap();

        let paths = PathsList {
            include: vec![
                PathEntry {
                    path: ".bashrc".to_string(),
                    merge_type: MergeType::None,
                },
                PathEntry {
                    path: ".vimrc".to_string(),
                    merge_type: MergeType::None,
                },
                PathEntry {
                    path: ".ssh/config".to_string(),
                    merge_type: MergeType::None,
                },
            ],
            exclude: vec![".ssh/id_*".to_string()],
        };

        let files = collect_files(home, &paths).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();

        assert!(names.contains(&".bashrc"));
        assert!(names.contains(&".vimrc"));
        assert!(names.contains(&".ssh/config"));
        assert!(!names.contains(&".ssh/id_rsa"));
    }

    #[test]
    fn collect_directory_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin/script1.sh"), "#!/bin/bash").unwrap();
        std::fs::write(home.join("bin/script2.sh"), "#!/bin/bash").unwrap();

        let paths = PathsList {
            include: vec![PathEntry {
                path: "bin/".to_string(),
                merge_type: MergeType::None,
            }],
            exclude: vec![],
        };

        let files = collect_files(home, &paths).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn checksum_consistency() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let hash1 = compute_checksum(&file).unwrap();
        let hash2 = compute_checksum(&file).unwrap();
        assert_eq!(hash1, hash2);
        assert!(hash1.starts_with("sha256:"));
    }

    #[test]
    fn checksum_bytes_matches_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        let content = b"hello world";
        std::fs::write(&file, content).unwrap();

        let file_hash = compute_checksum(&file).unwrap();
        let bytes_hash = compute_checksum_bytes(content);
        assert_eq!(file_hash, bytes_hash);
    }
}
