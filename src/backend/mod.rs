pub mod local;
mod s3;
mod sftp;
mod git;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no snapshots found for profile '{0}'")]
    NoSnapshots(String),
    #[error("snapshot version {0} not found")]
    VersionNotFound(u64),
    #[error("version conflict: local version {local} is behind remote version {remote}")]
    VersionConflict { local: u64, remote: u64 },
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("backend not implemented: {0}")]
    NotImplemented(String),
    #[error("failed to persist file: {0}")]
    Persist(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub version: u64,
    pub timestamp: DateTime<Utc>,
    pub size_bytes: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendManifest {
    pub latest_version: u64,
    pub snapshots: Vec<SnapshotMeta>,
}

impl Default for BackendManifest {
    fn default() -> Self {
        BackendManifest {
            latest_version: 0,
            snapshots: Vec::new(),
        }
    }
}

pub trait StorageBackend {
    fn push(
        &self,
        profile: &str,
        data: &[u8],
        message: Option<&str>,
    ) -> Result<u64, BackendError>;

    fn pull(
        &self,
        profile: &str,
        version: Option<u64>,
    ) -> Result<(u64, Vec<u8>), BackendError>;

    fn list_versions(&self, profile: &str) -> Result<Vec<SnapshotMeta>, BackendError>;

    fn prune(&self, profile: &str, keep: usize) -> Result<(), BackendError>;

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError>;
}
