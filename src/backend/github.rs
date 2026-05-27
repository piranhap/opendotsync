use chrono::Utc;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{BackendError, BackendManifest, SnapshotMeta, StorageBackend};

pub struct GitHubBackend {
    repo: String,
    branch: String,
    cache_dir: PathBuf,
}

impl GitHubBackend {
    pub fn new(repo: String, branch: String) -> Self {
        let cache_dir = Self::cache_dir_for_repo(&repo);
        GitHubBackend { repo, branch, cache_dir }
    }

    fn cache_dir_for_repo(repo: &str) -> PathBuf {
        let slug = repo
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("git@")
            .replace(['/', ':', '.'], "-");
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("opendotsync")
            .join("github-cache")
            .join(slug)
    }

    fn ensure_clone(&self) -> Result<(), BackendError> {
        if self.cache_dir.join(".git").exists() {
            // Sync with remote; ignore failures on an empty/new repo
            let _ = Command::new("git")
                .args([
                    "-C", self.cache_dir.to_str().unwrap_or(""),
                    "pull", "--ff-only", "origin", &self.branch,
                ])
                .status();
            return Ok(());
        }

        if let Some(parent) = self.cache_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Try cloning a specific branch first; fall back to default branch
        let ok = Command::new("git")
            .args(["clone", "--branch", &self.branch, &self.repo,
                   self.cache_dir.to_str().unwrap_or("")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !ok {
            let status = Command::new("git")
                .args(["clone", &self.repo, self.cache_dir.to_str().unwrap_or("")])
                .status()?;
            if !status.success() {
                return Err(BackendError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("git clone failed for {}", self.repo),
                )));
            }
        }
        Ok(())
    }

    fn profile_dir(&self, profile: &str) -> PathBuf {
        self.cache_dir.join(profile)
    }

    fn version_filename(version: u64) -> String {
        format!("v{:03}.tar.zst", version)
    }

    fn load_manifest(&self, profile: &str) -> Result<BackendManifest, BackendError> {
        let path = self.profile_dir(profile).join("manifest.json");
        if !path.exists() {
            return Ok(BackendManifest::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        serde_json::from_str(&contents).map_err(|e| BackendError::Manifest(e.to_string()))
    }

    fn save_manifest(&self, profile: &str, manifest: &BackendManifest) -> Result<(), BackendError> {
        let dir = self.profile_dir(profile);
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| BackendError::Manifest(e.to_string()))?;
        std::fs::write(dir.join("manifest.json"), json.as_bytes())?;
        Ok(())
    }

    fn commit_and_push(&self, message: &str) -> Result<(), BackendError> {
        let repo = self.cache_dir.to_str().unwrap_or("");

        let status = Command::new("git").args(["-C", repo, "add", "-A"]).status()?;
        if !status.success() {
            return Err(self.git_err("git add -A failed"));
        }

        // Only commit if there are staged changes
        let nothing_staged = Command::new("git")
            .args(["-C", repo, "diff", "--cached", "--quiet"])
            .status()?
            .success();

        if !nothing_staged {
            let status = Command::new("git")
                .args(["-C", repo, "commit", "-m", message])
                .status()?;
            if !status.success() {
                return Err(self.git_err("git commit failed"));
            }
        }

        let status = Command::new("git")
            .args(["-C", repo, "push", "origin", &self.branch])
            .status()?;
        if !status.success() {
            return Err(self.git_err("git push failed — check credentials and repo access"));
        }
        Ok(())
    }

    fn git_err(&self, msg: &str) -> BackendError {
        BackendError::Io(std::io::Error::new(std::io::ErrorKind::Other, msg))
    }

    fn init_repo_if_needed(&self) -> Result<(), BackendError> {
        if self.cache_dir.join(".git").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        let d = self.cache_dir.to_str().unwrap_or("");
        let run = |args: &[&str]| -> Result<(), BackendError> {
            if !Command::new("git").args(args).status()?.success() {
                return Err(self.git_err(&format!("git {} failed", args.join(" "))));
            }
            Ok(())
        };
        run(&["-C", d, "init", "-b", &self.branch])?;
        run(&["-C", d, "remote", "add", "origin", &self.repo])?;
        Ok(())
    }
}

impl StorageBackend for GitHubBackend {
    fn push(&self, profile: &str, data: &[u8], message: Option<&str>) -> Result<u64, BackendError> {
        if self.ensure_clone().is_err() {
            self.init_repo_if_needed()?;
        }

        let mut manifest = self.load_manifest(profile)?;
        let new_version = manifest.latest_version + 1;

        let profile_dir = self.profile_dir(profile);
        std::fs::create_dir_all(&profile_dir)?;
        std::fs::write(profile_dir.join(Self::version_filename(new_version)), data)?;

        manifest.latest_version = new_version;
        manifest.snapshots.push(SnapshotMeta {
            version: new_version,
            timestamp: Utc::now(),
            size_bytes: data.len() as u64,
            message: message.map(String::from),
        });
        self.save_manifest(profile, &manifest)?;

        let msg = format!(
            "opendotsync: {} v{:03}",
            message.unwrap_or("snapshot"),
            new_version
        );
        self.commit_and_push(&msg)?;
        Ok(new_version)
    }

    fn pull(&self, profile: &str, version: Option<u64>) -> Result<(u64, Vec<u8>), BackendError> {
        self.ensure_clone()?;
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
        let path = self.profile_dir(profile).join(Self::version_filename(target));
        if !path.exists() {
            return Err(BackendError::VersionNotFound(target));
        }
        Ok((target, std::fs::read(&path)?))
    }

    fn list_versions(&self, profile: &str) -> Result<Vec<SnapshotMeta>, BackendError> {
        self.ensure_clone()?;
        Ok(self.load_manifest(profile)?.snapshots)
    }

    fn prune(&self, profile: &str, keep: usize) -> Result<(), BackendError> {
        self.ensure_clone()?;
        let mut manifest = self.load_manifest(profile)?;
        if manifest.snapshots.len() <= keep {
            return Ok(());
        }
        manifest.snapshots.sort_by_key(|s| s.version);
        let to_remove = manifest.snapshots.len() - keep;
        for snap in &manifest.snapshots[..to_remove] {
            let path = self.profile_dir(profile).join(Self::version_filename(snap.version));
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        manifest.snapshots = manifest.snapshots[to_remove..].to_vec();
        self.save_manifest(profile, &manifest)?;
        self.commit_and_push("opendotsync: prune old snapshots")
    }

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError> {
        self.ensure_clone()?;
        let manifest = self.load_manifest(profile)?;
        Ok(if manifest.latest_version == 0 { None } else { Some(manifest.latest_version) })
    }
}
