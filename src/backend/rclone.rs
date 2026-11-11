use chrono::Utc;
use std::process::Command;
use tempfile::NamedTempFile;

use super::{
    run_captured, run_checked, BackendError, BackendManifest, SnapshotMeta, SnapshotOptions,
    StorageBackend,
};

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
        serde_json::from_slice(&output.stdout).map_err(|e| BackendError::Manifest(e.to_string()))
    }

    fn save_manifest(&self, profile: &str, manifest: &BackendManifest) -> Result<(), BackendError> {
        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;
        let tmp = NamedTempFile::new()?;
        std::fs::write(tmp.path(), json.as_bytes())?;
        let remote = self.remote_path(profile, "manifest.json");
        run_checked(
            Command::new("rclone").args(["copyto", tmp.path().to_str().unwrap_or(""), &remote]),
            "rclone copyto failed writing manifest",
        )?;
        Ok(())
    }
}

impl StorageBackend for RcloneBackend {
    fn check_connectivity(&self) -> Result<(), BackendError> {
        run_checked(
            Command::new("rclone").args([
                "lsd",
                &format!("{}:{}", self.remote, self.path),
                "--contimeout",
                "10s",
                "--timeout",
                "30s",
                "--retries",
                "1",
            ]),
            "remote access failed — check rclone configuration and credentials",
        )?;
        Ok(())
    }

    fn diagnostic_version(&self, profile: &str) -> Result<Option<u64>, BackendError> {
        let output = Command::new("rclone")
            .args([
                "cat",
                &self.remote_path(profile, "manifest.json"),
                "--contimeout",
                "10s",
                "--timeout",
                "30s",
                "--retries",
                "1",
            ])
            .output()?;
        // rclone uses exit code 3/4 for a missing directory/file.
        if matches!(output.status.code(), Some(3 | 4)) {
            return Ok(None);
        }
        if !output.status.success() {
            return Err(BackendError::Io(std::io::Error::other(format!(
                "reading remote manifest failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))));
        }
        let manifest: BackendManifest = serde_json::from_slice(&output.stdout)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;
        Ok((manifest.latest_version != 0).then_some(manifest.latest_version))
    }

    fn push(&self, profile: &str, data: &[u8], message: Option<&str>) -> Result<u64, BackendError> {
        self.push_snapshot(profile, data, message, SnapshotOptions::default(), None)
    }

    fn push_snapshot(
        &self,
        profile: &str,
        data: &[u8],
        message: Option<&str>,
        options: SnapshotOptions,
        expected_version: Option<u64>,
    ) -> Result<u64, BackendError> {
        let mut manifest = self.load_manifest(profile)?;
        manifest.validate_push(expected_version)?;
        let new_version = manifest.latest_version + 1;

        let tmp = NamedTempFile::new()?;
        std::fs::write(tmp.path(), data)?;
        let remote = self.remote_path(profile, &Self::version_filename(new_version));
        run_checked(
            Command::new("rclone").args(["copyto", tmp.path().to_str().unwrap_or(""), &remote]),
            &format!("rclone copyto failed for v{:03}", new_version),
        )?;

        manifest.latest_version = new_version;
        manifest.snapshots.push(SnapshotMeta {
            version: new_version,
            timestamp: Utc::now(),
            size_bytes: data.len() as u64,
            message: message.map(String::from),
            snapshot_type: options.snapshot_type,
            base_version: options.base_version,
            deleted_files: options.deleted_files,
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
        let to_remove = manifest.prune_boundary(keep);
        if to_remove == 0 {
            return Ok(());
        }
        for snap in &manifest.snapshots[..to_remove] {
            let remote = self.remote_path(profile, &Self::version_filename(snap.version));
            let _ = run_captured(Command::new("rclone").args(["deletefile", &remote]));
        }
        manifest.snapshots = manifest.snapshots[to_remove..].to_vec();
        self.save_manifest(profile, &manifest)
    }

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError> {
        let manifest = self.load_manifest(profile)?;
        Ok(if manifest.latest_version == 0 {
            None
        } else {
            Some(manifest.latest_version)
        })
    }
}
