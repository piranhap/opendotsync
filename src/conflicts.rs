use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConflictError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn stash_conflict(
    state_dir: &Path,
    version: u64,
    relative_path: &str,
    content: &[u8],
) -> Result<PathBuf, ConflictError> {
    let conflict_dir = state_dir.join("conflicts").join(format!("v{version}"));

    let conflict_path = conflict_dir.join(relative_path);

    if let Some(parent) = conflict_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&conflict_path, content)?;

    Ok(conflict_path)
}

pub fn list_conflict_versions(state_dir: &Path) -> Result<Vec<u64>, ConflictError> {
    let conflicts_dir = state_dir.join("conflicts");
    if !conflicts_dir.exists() {
        return Ok(Vec::new());
    }

    let mut versions = Vec::new();
    for entry in std::fs::read_dir(&conflicts_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(stripped) = name_str.strip_prefix('v') {
            if let Ok(version) = stripped.parse::<u64>() {
                versions.push(version);
            }
        }
    }

    versions.sort();
    Ok(versions)
}

pub fn list_conflicts_for_version(
    state_dir: &Path,
    version: u64,
) -> Result<Vec<String>, ConflictError> {
    let version_dir = state_dir.join("conflicts").join(format!("v{version}"));

    if !version_dir.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    collect_files_recursive(&version_dir, &version_dir, &mut paths)?;

    paths.sort();
    Ok(paths)
}

fn collect_files_recursive(
    base: &Path,
    current: &Path,
    paths: &mut Vec<String>,
) -> Result<(), ConflictError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(base, &path, paths)?;
        } else {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            paths.push(relative);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_and_list_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path();

        stash_conflict(state_dir, 5, ".bashrc", b"remote bashrc").unwrap();
        stash_conflict(state_dir, 5, ".vimrc", b"remote vimrc").unwrap();
        stash_conflict(state_dir, 8, ".bashrc", b"another conflict").unwrap();

        let versions = list_conflict_versions(state_dir).unwrap();
        assert_eq!(versions, vec![5, 8]);

        let files = list_conflicts_for_version(state_dir, 5).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.contains(&".bashrc".to_string()));
        assert!(files.contains(&".vimrc".to_string()));

        let files_v8 = list_conflicts_for_version(state_dir, 8).unwrap();
        assert_eq!(files_v8, vec![".bashrc".to_string()]);
    }

    #[test]
    fn no_conflicts_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let versions = list_conflict_versions(dir.path()).unwrap();
        assert!(versions.is_empty());
    }
}
