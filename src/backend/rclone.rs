use chrono::Utc;
use std::process::Command;
use tempfile::NamedTempFile;

use super::{BackendError, BackendManifest, SnapshotMeta, StorageBackend};

pub struct RcloneBackend {
    remote: String,
    path: String,
}

impl RcloneBackend {
    pub fn new(remote: String, path: String) -> Self {
        RcloneBackend { remote, path }
    }

    fn remote_path(&self, profile: &str, filename: &str) -> String {
        format!("{}:{}/{}/{}", self.remote, self.path, profile, filename)
    }

    fn version_filename(version: u64) -> String {
        format!("v{:03}.tar.zst", version)
    }

    fn load_manifest(&self, profile: &str) -> Result<BackendManifest, BackendError> {
        let remote = self.remote_path(profile, "manifest.json");
        let output = Command::new("rclone").args(["cat", &remote]).output()?;
        if !output.status.success() || output.stdout.is_empty() {
            return Ok(BackendManifest::default());
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| BackendError::Manifest(e.to_string()))
    }

    fn save_manifest(&self, profile: &str, manifest: &BackendManifest) -> Result<(), BackendError> {
        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;
        let tmp = NamedTempFile::new()?;
        std::fs::write(tmp.path(), json.as_bytes())?;
        let remote = self.remote_path(profile, "manifest.json");
        let status = Command::new("rclone")
            .args(["copyto", tmp.path().to_str().unwrap_or(""), &remote])
            .status()?;
        if !status.success() {
            return Err(BackendError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "rclone copyto failed writing manifest",
            )));
        }
        Ok(())
    }
}

impl StorageBackend for RcloneBackend {
    fn push(&self, profile: &str, data: &[u8], message: Option<&str>) -> Result<u64, BackendError> {
        let mut manifest = self.load_manifest(profile)?;
        let new_version = manifest.latest_version + 1;

        let tmp = NamedTempFile::new()?;
        std::fs::write(tmp.path(), data)?;
        let remote = self.remote_path(profile, &Self::version_filename(new_version));
        let status = Command::new("rclone")
            .args(["copyto", tmp.path().to_str().unwrap_or(""), &remote])
            .status()?;
        if !status.success() {
            return Err(BackendError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("rclone copyto failed for v{:03}", new_version),
            )));
        }

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

    fn pull(&self, profile: &str, version: Option<u64>) -> Result<(u64, Vec<u8>), BackendError> {
        let manifest = self.load_manifest(profile)?;
        let target = match version {
            Some(v) => v,
            None => {
                if manifest.latest_version == 0 {
                    return Err(BackendError::NoSnapshots(profile.to_string()));
                }
                manifest.latest_version
            }
        };
        let remote = self.remote_path(profile, &Self::version_filename(target));
        let output = Command::new("rclone").args(["cat", &remote]).output()?;
        if !output.status.success() {
            return Err(BackendError::VersionNotFound(target));
        }
        Ok((target, output.stdout))
    }

    fn list_versions(&self, profile: &str) -> Result<Vec<SnapshotMeta>, BackendError> {
        Ok(self.load_manifest(profile)?.snapshots)
    }

    fn prune(&self, profile: &str, keep: usize) -> Result<(), BackendError> {
        let mut manifest = self.load_manifest(profile)?;
        if manifest.snapshots.len() <= keep {
            return Ok(());
        }
        manifest.snapshots.sort_by_key(|s| s.version);
        let to_remove = manifest.snapshots.len() - keep;
        for snap in &manifest.snapshots[..to_remove] {
            let remote = self.remote_path(profile, &Self::version_filename(snap.version));
            let _ = Command::new("rclone").args(["deletefile", &remote]).status();
        }
        manifest.snapshots = manifest.snapshots[to_remove..].to_vec();
        self.save_manifest(profile, &manifest)
    }

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError> {
        let manifest = self.load_manifest(profile)?;
        Ok(if manifest.latest_version == 0 { None } else { Some(manifest.latest_version) })
    }
}
