use std::path::PathBuf;

use chrono::Utc;

use super::{BackendError, BackendManifest, SnapshotMeta, StorageBackend};

pub struct LocalBackend {
    base_path: PathBuf,
}

impl LocalBackend {
    pub fn new(base_path: PathBuf) -> Self {
        LocalBackend { base_path }
    }

    fn profile_dir(&self, profile: &str) -> PathBuf {
        self.base_path.join(profile)
    }

    fn manifest_path(&self, profile: &str) -> PathBuf {
        self.profile_dir(profile).join("manifest.json")
    }

    fn load_manifest(&self, profile: &str) -> Result<BackendManifest, BackendError> {
        let path = self.manifest_path(profile);
        if !path.exists() {
            return Ok(BackendManifest::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        let manifest: BackendManifest = serde_json::from_str(&contents)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;
        Ok(manifest)
    }

    fn save_manifest(
        &self,
        profile: &str,
        manifest: &BackendManifest,
    ) -> Result<(), BackendError> {
        let path = self.manifest_path(profile);
        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;

        let dir = self.profile_dir(profile);
        std::fs::create_dir_all(&dir)?;

        let tmp = tempfile::NamedTempFile::new_in(&dir)?;
        std::fs::write(tmp.path(), json.as_bytes())?;
        tmp.persist(&path)
            .map_err(|e| BackendError::Persist(e.to_string()))?;

        Ok(())
    }

    fn version_filename(version: u64) -> String {
        format!("v{:03}.tar.zst", version)
    }

    fn snapshot_path(&self, profile: &str, version: u64) -> PathBuf {
        self.profile_dir(profile)
            .join(Self::version_filename(version))
    }
}

impl StorageBackend for LocalBackend {
    fn push(
        &self,
        profile: &str,
        data: &[u8],
        message: Option<&str>,
    ) -> Result<u64, BackendError> {
        let dir = self.profile_dir(profile);
        std::fs::create_dir_all(&dir)?;

        let mut manifest = self.load_manifest(profile)?;
        let new_version = manifest.latest_version + 1;

        // Atomic write: temp file then rename
        let snapshot_path = self.snapshot_path(profile, new_version);
        let tmp = tempfile::NamedTempFile::new_in(&dir)?;
        std::fs::write(tmp.path(), data)?;
        tmp.persist(&snapshot_path)
            .map_err(|e| BackendError::Persist(e.to_string()))?;

        manifest.latest_version = new_version;
        manifest.snapshots.push(SnapshotMeta {
            version: new_version,
            timestamp: Utc::now(),
            size_bytes: data.len() as u64,
            message: message.map(String::from),
        });

        self.save_manifest(profile, &manifest)?;

        Ok(new_version)
    }

    fn pull(
        &self,
        profile: &str,
        version: Option<u64>,
    ) -> Result<(u64, Vec<u8>), BackendError> {
        let manifest = self.load_manifest(profile)?;

        let target_version = match version {
            Some(v) => v,
            None => {
                if manifest.latest_version == 0 {
                    return Err(BackendError::NoSnapshots(profile.to_string()));
                }
                manifest.latest_version
            }
        };

        let snapshot_path = self.snapshot_path(profile, target_version);
        if !snapshot_path.exists() {
            return Err(BackendError::VersionNotFound(target_version));
        }

        let data = std::fs::read(&snapshot_path)?;
        Ok((target_version, data))
    }

    fn list_versions(&self, profile: &str) -> Result<Vec<SnapshotMeta>, BackendError> {
        let manifest = self.load_manifest(profile)?;
        Ok(manifest.snapshots)
    }

    fn prune(&self, profile: &str, keep: usize) -> Result<(), BackendError> {
        let mut manifest = self.load_manifest(profile)?;
        if manifest.snapshots.len() <= keep {
            return Ok(());
        }
        manifest.snapshots.sort_by_key(|s| s.version);
        let to_remove = manifest.snapshots.len() - keep;
        for snap in &manifest.snapshots[..to_remove] {
            let path = self.snapshot_path(profile, snap.version);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        manifest.snapshots = manifest.snapshots[to_remove..].to_vec();
        self.save_manifest(profile, &manifest)?;
        Ok(())
    }

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError> {
        let manifest = self.load_manifest(profile)?;
        if manifest.latest_version == 0 {
            Ok(None)
        } else {
            Ok(Some(manifest.latest_version))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_backend() -> (tempfile::TempDir, LocalBackend) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_path_buf());
        (dir, backend)
    }

    #[test]
    fn push_and_pull() {
        let (_dir, backend) = test_backend();
        let data = b"snapshot data here";

        let version = backend.push("test", data, Some("first push")).unwrap();
        assert_eq!(version, 1);

        let (pulled_version, pulled_data) = backend.pull("test", None).unwrap();
        assert_eq!(pulled_version, 1);
        assert_eq!(pulled_data, data);
    }

    #[test]
    fn push_increments_version() {
        let (_dir, backend) = test_backend();

        let v1 = backend.push("test", b"v1", None).unwrap();
        let v2 = backend.push("test", b"v2", None).unwrap();
        let v3 = backend.push("test", b"v3", None).unwrap();

        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(v3, 3);
    }

    #[test]
    fn pull_specific_version() {
        let (_dir, backend) = test_backend();

        backend.push("test", b"version-1-data", None).unwrap();
        backend.push("test", b"version-2-data", None).unwrap();

        let (v, data) = backend.pull("test", Some(1)).unwrap();
        assert_eq!(v, 1);
        assert_eq!(data, b"version-1-data");
    }

    #[test]
    fn latest_version_empty() {
        let (_dir, backend) = test_backend();
        let latest = backend.latest_version("test").unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn latest_version_after_push() {
        let (_dir, backend) = test_backend();
        backend.push("test", b"data", None).unwrap();
        backend.push("test", b"data2", None).unwrap();

        let latest = backend.latest_version("test").unwrap();
        assert_eq!(latest, Some(2));
    }

    #[test]
    fn list_versions() {
        let (_dir, backend) = test_backend();
        backend.push("test", b"a", Some("first")).unwrap();
        backend.push("test", b"bb", None).unwrap();

        let versions = backend.list_versions("test").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[0].size_bytes, 1);
        assert_eq!(versions[0].message.as_deref(), Some("first"));
        assert_eq!(versions[1].version, 2);
        assert_eq!(versions[1].size_bytes, 2);
        assert!(versions[1].message.is_none());
    }

    #[test]
    fn pull_nonexistent_version() {
        let (_dir, backend) = test_backend();
        backend.push("test", b"data", None).unwrap();

        let result = backend.pull("test", Some(99));
        assert!(result.is_err());
    }

    #[test]
    fn pull_empty_profile() {
        let (_dir, backend) = test_backend();
        let result = backend.pull("empty", None);
        assert!(result.is_err());
    }
}
