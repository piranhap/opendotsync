// S3-compatible backend — stub. Add `aws-sdk-s3` dep and implement when ready.

use super::{BackendError, SnapshotMeta, StorageBackend};

#[allow(dead_code)]
pub struct S3Backend;

impl StorageBackend for S3Backend {
    fn push(
        &self,
        _profile: &str,
        _data: &[u8],
        _message: Option<&str>,
    ) -> Result<u64, BackendError> {
        Err(BackendError::NotImplemented("s3".into()))
    }
    fn pull(&self, _profile: &str, _version: Option<u64>) -> Result<(u64, Vec<u8>), BackendError> {
        Err(BackendError::NotImplemented("s3".into()))
    }
    fn list_versions(&self, _profile: &str) -> Result<Vec<SnapshotMeta>, BackendError> {
        Err(BackendError::NotImplemented("s3".into()))
    }
    fn prune(&self, _profile: &str, _keep: usize) -> Result<(), BackendError> {
        Err(BackendError::NotImplemented("s3".into()))
    }
    fn latest_version(&self, _profile: &str) -> Result<Option<u64>, BackendError> {
        Err(BackendError::NotImplemented("s3".into()))
    }
}
