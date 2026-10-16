use chrono::Utc;
use std::path::PathBuf;
use std::process::Command;

use super::{
    run_captured, run_checked, BackendError, BackendManifest, SnapshotMeta, StorageBackend,
};

pub struct GitHubBackend {
    repo: String,
    branch: String,
    cache_dir: PathBuf,
}

impl GitHubBackend {
    pub fn new(repo: String, branch: String) -> Self {
        // Never retain credentials in the struct — they would leak into the cache
        // path and .git/config. Auth is resolved at runtime from the environment.
        let repo = Self::strip_credentials(&repo);
        let cache_dir = Self::cache_dir_for_repo(&repo);
        GitHubBackend {
            repo,
            branch,
            cache_dir,
        }
    }

    /// Remove a `user[:password]@` userinfo component from an http(s) URL.
    /// SSH-form URLs like `git@github.com:user/repo.git` are left untouched —
    /// there the `git@` is the SSH user, not a secret.
    pub fn strip_credentials(repo: &str) -> String {
        for scheme in ["https://", "http://"] {
            if let Some(rest) = repo.strip_prefix(scheme) {
                if let Some(at) = rest.find('@') {
                    return format!("{}{}", scheme, &rest[at + 1..]);
                }
                return repo.to_string();
            }
        }
        repo.to_string()
    }

    /// True if the URL carries an embedded `user@`/`user:pass@` credential.
    pub fn has_embedded_credentials(repo: &str) -> bool {
        for scheme in ["https://", "http://"] {
            if let Some(rest) = repo.strip_prefix(scheme) {
                return rest.contains('@');
            }
        }
        false
    }

    /// Resolve a GitHub token from the environment, if present and non-empty.
    fn token() -> Option<String> {
        std::env::var("OPENDOTSYNC_GITHUB_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
    }

    /// A transient, authenticated URL for network operations. The token (from
    /// the environment) is injected only into the argument passed to a single
    /// git invocation — it is never written to config.yaml or .git/config.
    /// Falls back to the clean URL (letting git's own credential helpers /
    /// ~/.git-credentials handle auth) when no token is set or the URL is SSH.
    fn auth_url(&self) -> String {
        match Self::token() {
            Some(tok) if self.repo.starts_with("https://") => {
                format!(
                    "https://x-access-token:{}@{}",
                    tok,
                    &self.repo["https://".len()..]
                )
            }
            _ => self.repo.clone(),
        }
    }

    pub fn validate_repo_url(repo: &str) -> Result<(), BackendError> {
        // Catch mixed https://git@ format (a common mistake)
        if repo.starts_with("https://git@") || repo.starts_with("http://git@") {
            return Err(BackendError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "invalid repo URL '{}'\n  Use SSH:   git@github.com:user/repo.git\n  Use HTTPS: https://github.com/user/repo.git",
                    repo
                ),
            )));
        }
        Ok(())
    }

    fn cache_dir_for_repo(repo: &str) -> PathBuf {
        // Strip any embedded credential first so a token can never appear in the
        // on-disk cache directory name.
        let clean = Self::strip_credentials(repo);
        let slug = clean
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
        let auth_url = self.auth_url();
        if self.cache_dir.join(".git").exists() {
            // Sync with remote via an explicit (possibly authenticated) URL so the
            // token is never persisted in .git/config. Ignore failures on a new repo.
            let _ = run_captured(Command::new("git").args([
                "-C",
                self.cache_dir.to_str().unwrap_or(""),
                "pull",
                "--ff-only",
                &auth_url,
                &self.branch,
            ]));
            return Ok(());
        }

        if let Some(parent) = self.cache_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Try cloning a specific branch first; fall back to default branch
        let ok = run_captured(Command::new("git").args([
            "clone",
            "--branch",
            &self.branch,
            &auth_url,
            self.cache_dir.to_str().unwrap_or(""),
        ]))
        .map(|o| o.status.success())
        .unwrap_or(false);

        if !ok {
            run_checked(
                Command::new("git").args([
                    "clone",
                    &auth_url,
                    self.cache_dir.to_str().unwrap_or(""),
                ]),
                &format!("git clone failed for {}", self.repo),
            )?;
        }

        // `git clone` records the (authenticated) clone URL as origin. Reset it to
        // the credential-free URL so the token is not left behind in .git/config.
        let _ = run_captured(Command::new("git").args([
            "-C",
            self.cache_dir.to_str().unwrap_or(""),
            "remote",
            "set-url",
            "origin",
            &self.repo,
        ]));
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

    fn ensure_git_identity(&self) {
        let repo = self.cache_dir.to_str().unwrap_or("");
        let has_email = Command::new("git")
            .args(["-C", repo, "config", "user.email"])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);
        if !has_email {
            let _ = run_captured(Command::new("git").args([
                "-C",
                repo,
                "config",
                "user.email",
                "opendotsync@localhost",
            ]));
            let _ = run_captured(Command::new("git").args([
                "-C",
                repo,
                "config",
                "user.name",
                "opendotsync",
            ]));
        }
    }

    fn commit_and_push(&self, message: &str) -> Result<(), BackendError> {
        let repo = self.cache_dir.to_str().unwrap_or("");

        self.ensure_git_identity();

        run_checked(
            Command::new("git").args(["-C", repo, "add", "-A"]),
            "git add -A failed",
        )?;

        // Only commit if there are staged changes
        let nothing_staged =
            run_captured(Command::new("git").args(["-C", repo, "diff", "--cached", "--quiet"]))?
                .status
                .success();

        if !nothing_staged {
            run_checked(
                Command::new("git").args(["-C", repo, "commit", "-m", message]),
                "git commit failed",
            )?;
        }

        // Push to an explicit (possibly authenticated) URL rather than "origin"
        // so the token is never persisted in .git/config.
        let auth_url = self.auth_url();
        run_checked(
            Command::new("git").args(["-C", repo, "push", &auth_url, &self.branch]),
            "git push failed — check credentials and repo access",
        )?;
        Ok(())
    }

    fn init_repo_if_needed(&self) -> Result<(), BackendError> {
        if self.cache_dir.join(".git").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        let d = self.cache_dir.to_str().unwrap_or("");
        let run = |args: &[&str]| -> Result<(), BackendError> {
            run_checked(
                Command::new("git").args(args),
                &format!("git {} failed", args.join(" ")),
            )?;
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
        let path = self
            .profile_dir(profile)
            .join(Self::version_filename(target));
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
            let path = self
                .profile_dir(profile)
                .join(Self::version_filename(snap.version));
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
        Ok(if manifest.latest_version == 0 {
            None
        } else {
            Some(manifest.latest_version)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_credentials_https_token() {
        assert_eq!(
            GitHubBackend::strip_credentials("https://ghp_SECRET@github.com/u/r.git"),
            "https://github.com/u/r.git"
        );
        assert_eq!(
            GitHubBackend::strip_credentials("https://user:ghp_SECRET@github.com/u/r.git"),
            "https://github.com/u/r.git"
        );
    }

    #[test]
    fn strip_credentials_leaves_clean_and_ssh_urls() {
        // No credentials → unchanged.
        assert_eq!(
            GitHubBackend::strip_credentials("https://github.com/u/r.git"),
            "https://github.com/u/r.git"
        );
        // SSH form: `git@` is the SSH user, not a secret — leave it alone.
        assert_eq!(
            GitHubBackend::strip_credentials("git@github.com:u/r.git"),
            "git@github.com:u/r.git"
        );
    }

    #[test]
    fn has_embedded_credentials_detects_token() {
        assert!(GitHubBackend::has_embedded_credentials(
            "https://ghp_SECRET@github.com/u/r.git"
        ));
        assert!(!GitHubBackend::has_embedded_credentials(
            "https://github.com/u/r.git"
        ));
        // SSH form is not treated as embedded HTTP credentials.
        assert!(!GitHubBackend::has_embedded_credentials(
            "git@github.com:u/r.git"
        ));
    }

    #[test]
    fn cache_dir_never_contains_token() {
        let dir = GitHubBackend::cache_dir_for_repo("https://ghp_SECRET@github.com/u/r.git");
        let s = dir.to_string_lossy();
        assert!(
            !s.contains("ghp_SECRET"),
            "token leaked into cache path: {s}"
        );
    }

    #[test]
    fn new_does_not_retain_token_in_repo_or_cache() {
        let b = GitHubBackend::new(
            "https://ghp_SECRET@github.com/u/r.git".to_string(),
            "main".to_string(),
        );
        assert_eq!(b.repo, "https://github.com/u/r.git");
        assert!(!b.cache_dir.to_string_lossy().contains("ghp_SECRET"));
    }
}
