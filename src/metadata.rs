use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("failed to parse metadata: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to persist temp file: {0}")]
    Persist(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub profile: String,
    pub version: u64,
    pub machine_id: String,
    pub last_push: Option<DateTime<Utc>>,
    pub last_pull: Option<DateTime<Utc>>,
    pub checksums: HashMap<String, String>,
}

pub fn state_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("opendotsync")
}

pub fn metadata_path() -> PathBuf {
    state_dir().join("metadata.json")
}

pub fn parent_snapshot_path() -> PathBuf {
    state_dir().join("parent.tar.zst")
}

pub fn conflicts_dir() -> PathBuf {
    state_dir().join("conflicts")
}

pub fn load_metadata() -> Result<Option<Metadata>, MetadataError> {
    let path = metadata_path();
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)?;
    let meta: Metadata = serde_json::from_str(&contents)?;
    Ok(Some(meta))
}

pub fn load_metadata_from(path: &Path) -> Result<Option<Metadata>, MetadataError> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)?;
    let meta: Metadata = serde_json::from_str(&contents)?;
    Ok(Some(meta))
}

pub fn save_metadata(meta: &Metadata) -> Result<(), MetadataError> {
    save_metadata_to(meta, &metadata_path())
}

pub fn save_metadata_to(meta: &Metadata, path: &Path) -> Result<(), MetadataError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(meta)?;

    let dir = path
        .parent()
        .ok_or_else(|| MetadataError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "metadata path has no parent directory",
        )))?;

    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(tmp.path(), json.as_bytes())?;
    tmp.persist(path)
        .map_err(|e| MetadataError::Persist(e.to_string()))?;

    Ok(())
}

pub fn create_initial(profile: &str) -> Metadata {
    Metadata {
        profile: profile.to_string(),
        version: 0,
        machine_id: generate_machine_id(),
        last_push: None,
        last_pull: None,
        checksums: HashMap::new(),
    }
}

pub fn generate_machine_id() -> String {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let year = Utc::now().format("%Y");
    format!("{hostname}-{year}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.json");

        let meta = Metadata {
            profile: "test".to_string(),
            version: 42,
            machine_id: "test-machine-2026".to_string(),
            last_push: Some(Utc::now()),
            last_pull: None,
            checksums: HashMap::from([
                (".bashrc".to_string(), "sha256:abc123".to_string()),
                (".vimrc".to_string(), "sha256:def456".to_string()),
            ]),
        };

        save_metadata_to(&meta, &path).unwrap();
        let loaded = load_metadata_from(&path).unwrap().unwrap();

        assert_eq!(loaded.profile, "test");
        assert_eq!(loaded.version, 42);
        assert_eq!(loaded.machine_id, "test-machine-2026");
        assert_eq!(loaded.checksums.len(), 2);
        assert_eq!(loaded.checksums[".bashrc"], "sha256:abc123");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let path = Path::new("/tmp/nonexistent-metadata-test.json");
        let result = load_metadata_from(path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn create_initial_metadata() {
        let meta = create_initial("personal");
        assert_eq!(meta.profile, "personal");
        assert_eq!(meta.version, 0);
        assert!(!meta.machine_id.is_empty());
        assert!(meta.checksums.is_empty());
    }
}
