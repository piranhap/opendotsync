use std::cmp::Reverse;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::backend::github::GitHubBackend;
use crate::backend::local::LocalBackend;
use crate::backend::rclone::RcloneBackend;
use crate::backend::{BackendError, StorageBackend};
use crate::retention;
use crate::collector::{self, CollectedFile};
use crate::config::{AppConfig, BackendConfig, ConfigError};
use crate::conflicts;
use crate::encryption;
use crate::merge::{self, MergeResult};
use crate::metadata::{self, Metadata, MetadataError};
use crate::paths::{self, PathsList, PathsError};
use crate::snapshot::{self, SnapshotError};

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("paths error: {0}")]
    Paths(#[from] PathsError),
    #[error("metadata error: {0}")]
    Metadata(#[from] MetadataError),
    #[error("collector error: {0}")]
    Collector(#[from] crate::collector::CollectorError),
    #[error("snapshot error: {0}")]
    Snapshot(#[from] SnapshotError),
    #[error("backend error: {0}")]
    Backend(#[from] BackendError),
    #[error("merge error: {0}")]
    Merge(#[from] merge::MergeError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported backend type: {0}")]
    UnsupportedBackend(String),
    #[error("not initialized — run `opendotsync init` first")]
    NotInitialized,
    #[error("snapshot exceeds size limit: {0} bytes")]
    SizeLimit(u64),
    #[error("encryption error: {0}")]
    Encryption(#[from] crate::encryption::EncryptionError),
    #[error("profile error: {0}")]
    Profile(#[from] crate::profiles::ProfileError),
}

pub struct PullResult {
    pub version: u64,
    pub updated: Vec<String>,
    pub conflicts: Vec<String>,
    pub new_files: Vec<String>,
    pub already_up_to_date: bool,
}

impl fmt::Display for PullResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.already_up_to_date {
            return write!(f, "Already up to date (version {})", self.version);
        }

        writeln!(f, "Pulled version {}", self.version)?;

        if !self.updated.is_empty() {
            writeln!(f, "Updated:")?;
            for path in &self.updated {
                writeln!(f, "  {path}")?;
            }
        }

        if !self.new_files.is_empty() {
            writeln!(f, "New files:")?;
            for path in &self.new_files {
                writeln!(f, "  {path}")?;
            }
        }

        if !self.conflicts.is_empty() {
            writeln!(f, "Conflicts (remote stashed):")?;
            for path in &self.conflicts {
                writeln!(f, "  {path}")?;
            }
        }

        Ok(())
    }
}

pub struct StatusReport {
    pub profile: String,
    pub local_version: u64,
    pub remote_version: Option<u64>,
    pub changed_files: Vec<String>,
    pub pending_conflicts: Vec<u64>,
}

impl fmt::Display for StatusReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Profile: {}", self.profile)?;
        writeln!(f, "Local version: {}", self.local_version)?;
        match self.remote_version {
            Some(v) => writeln!(f, "Remote version: {v}")?,
            None => writeln!(f, "Remote version: (none)")?,
        };

        if self.changed_files.is_empty() {
            writeln!(f, "No local changes since last push")?;
        } else {
            writeln!(f, "Changed files ({}):", self.changed_files.len())?;
            for path in &self.changed_files {
                writeln!(f, "  M {path}")?;
            }
        }

        if !self.pending_conflicts.is_empty() {
            writeln!(
                f,
                "Pending conflicts in versions: {:?}",
                self.pending_conflicts
            )?;
        }

        Ok(())
    }
}

pub struct DuReport {
    pub files: Vec<(String, u64)>,
    pub total_bytes: u64,
    pub warnings: Vec<String>,
}

impl fmt::Display for DuReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Projected snapshot contents:")?;
        for (path, size) in &self.files {
            writeln!(f, "  {:>8}  {path}", format_size(*size))?;
        }
        writeln!(f, "  {:>8}  (total)", format_size(self.total_bytes))?;

        for warning in &self.warnings {
            writeln!(f, "  WARNING: {warning}")?;
        }

        Ok(())
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn create_backend(config: &AppConfig) -> Result<Box<dyn StorageBackend>, SyncError> {
    match &config.backend {
        BackendConfig::Local { path } => Ok(Box::new(LocalBackend::new(path.clone()))),
        BackendConfig::Rclone { remote, path } => {
            Ok(Box::new(RcloneBackend::new(remote.clone(), path.clone())))
        }
        BackendConfig::GitHub { repo, branch } => {
            Ok(Box::new(GitHubBackend::new(repo.clone(), branch.clone())))
        }
        BackendConfig::S3 { .. } => Err(SyncError::UnsupportedBackend("s3".to_string())),
        BackendConfig::Sftp { .. } => Err(SyncError::UnsupportedBackend("sftp".to_string())),
        BackendConfig::Git { .. } => Err(SyncError::UnsupportedBackend("git".to_string())),
    }
}

pub fn cmd_init(
    home_dir: &Path,
    backend_type: &str,
    backend_path: Option<&Path>,
    profile: &str,
) -> Result<(), SyncError> {
    let backend_config = match backend_type {
        "local" => {
            let path = backend_path
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir.join(".opendotsync-store"));
            BackendConfig::Local { path }
        }
        "rclone" => {
            let remote = std::env::var("OPENDOTSYNC_RCLONE_REMOTE")
                .unwrap_or_else(|_| "myremote".to_string());
            let path = backend_path
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "opendotsync-store".to_string());
            BackendConfig::Rclone { remote, path }
        }
        "github" => {
            let repo = backend_path
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| SyncError::UnsupportedBackend(
                    "github backend requires --path <repo-url>".to_string(),
                ))?;
            BackendConfig::GitHub { repo, branch: "main".to_string() }
        }
        other => return Err(SyncError::UnsupportedBackend(other.to_string())),
    };

    // Write config
    let config_path = crate::config::default_config_path();
    crate::config::write_default_config(&config_path, &backend_config, profile)?;
    println!("Created config: {}", config_path.display());

    // Write paths list
    let paths_path = paths::default_paths_path();
    paths::write_default_paths(&paths_path)?;
    println!("Created paths list: {}", paths_path.display());

    // Create state directory
    let state_dir = metadata::state_dir();
    std::fs::create_dir_all(&state_dir)?;

    // Initialize metadata
    let meta = metadata::create_initial(profile);
    metadata::save_metadata(&meta)?;
    println!("Created metadata: {}", metadata::metadata_path().display());

    // Load what we just created and do initial push
    let config = crate::config::load_config(Some(&config_path))?;
    let paths_list = paths::load_paths(Some(&paths_path))?;

    let files = collector::collect_files(home_dir, &paths_list)?;

    if files.is_empty() {
        println!("No files matched — check your paths list");
        return Ok(());
    }

    let warnings = collector::check_size_limits(&files);
    for w in &warnings {
        println!("WARNING: {}", w.message);
    }

    let snapshot_data = snapshot::create_snapshot(home_dir, &files)?;
    let snapshot_data = encryption::encrypt(&snapshot_data, &config.encryption)?;

    let backend = create_backend(&config)?;
    let version = backend.push(profile, &snapshot_data, Some("initial snapshot"))?;

    // Save parent snapshot
    let parent_path = metadata::parent_snapshot_path();
    std::fs::write(&parent_path, &snapshot_data)?;

    // Update metadata with checksums
    let checksums = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.checksum.clone()))
        .collect();

    let meta = Metadata {
        profile: profile.to_string(),
        version,
        machine_id: metadata::generate_machine_id(),
        last_push: Some(Utc::now()),
        last_pull: None,
        checksums,
    };
    metadata::save_metadata(&meta)?;

    println!(
        "Initialized opendotsync: {} files, version {version}",
        files.len()
    );
    Ok(())
}

pub fn cmd_push(
    config: &AppConfig,
    paths_list: &PathsList,
    home_dir: &Path,
    force: bool,
    message: Option<&str>,
) -> Result<u64, SyncError> {
    let meta = metadata::load_metadata()?
        .ok_or(SyncError::NotInitialized)?;

    let files = collector::collect_files(home_dir, paths_list)?;

    // Check if anything changed
    let current_checksums: std::collections::HashMap<String, String> = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.checksum.clone()))
        .collect();

    if current_checksums == meta.checksums && !force {
        println!("No changes to push");
        return Ok(meta.version);
    }

    let warnings = collector::check_size_limits(&files);
    for w in &warnings {
        eprintln!("WARNING: {}", w.message);
    }

    let total = collector::total_size(&files);
    if total > 500 * 1024 * 1024 {
        return Err(SyncError::SizeLimit(total));
    }

    let snapshot_data = snapshot::create_snapshot(home_dir, &files)?;
    let snapshot_data = encryption::encrypt(&snapshot_data, &config.encryption)?;

    let backend = create_backend(config)?;

    // Check for version conflicts
    if !force {
        if let Some(remote_version) = backend.latest_version(&meta.profile)? {
            if remote_version > meta.version {
                return Err(SyncError::Backend(BackendError::VersionConflict {
                    local: meta.version,
                    remote: remote_version,
                }));
            }
        }
    }

    let new_version = backend.push(&meta.profile, &snapshot_data, message)?;

    let _ = retention::prune(backend.as_ref(), &meta.profile, retention::DEFAULT_KEEP);

    // Save as parent snapshot
    let parent_path = metadata::parent_snapshot_path();
    std::fs::write(&parent_path, &snapshot_data)?;

    // Update metadata
    let updated_meta = Metadata {
        version: new_version,
        last_push: Some(Utc::now()),
        checksums: current_checksums,
        ..meta
    };
    metadata::save_metadata(&updated_meta)?;

    Ok(new_version)
}

pub fn cmd_pull(
    config: &AppConfig,
    paths_list: &PathsList,
    home_dir: &Path,
    version: Option<u64>,
    overwrite: bool,
) -> Result<PullResult, SyncError> {
    let meta = metadata::load_metadata()?
        .ok_or(SyncError::NotInitialized)?;

    let backend = create_backend(config)?;
    let (remote_version, remote_data) = backend.pull(&meta.profile, version)?;

    if remote_version <= meta.version && !overwrite {
        return Ok(PullResult {
            version: meta.version,
            updated: vec![],
            conflicts: vec![],
            new_files: vec![],
            already_up_to_date: true,
        });
    }

    let remote_data = encryption::decrypt(&remote_data, &config.encryption)?;

    let remote_snapshot = snapshot::extract_snapshot(&remote_data)?;

    // Load parent snapshot for ancestor comparison
    let parent_path = metadata::parent_snapshot_path();
    let parent_data = if parent_path.exists() {
        Some(std::fs::read(&parent_path)?)
    } else {
        None
    };

    let parent_data = match &parent_data {
        Some(data) => Some(encryption::decrypt(data, &config.encryption)?),
        None => None,
    };

    let remote_files = snapshot::list_snapshot_contents(&remote_data)?;
    let state_dir = metadata::state_dir();

    let mut updated = Vec::new();
    let mut conflict_paths = Vec::new();
    let mut new_files = Vec::new();

    for remote_file_path in &remote_files {
        let local_path = home_dir.join(remote_file_path);

        let remote_content = std::fs::read(remote_snapshot.path().join(remote_file_path))?;

        if overwrite {
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&local_path, &remote_content)?;
            updated.push(remote_file_path.clone());
            continue;
        }

        let ancestor_content = match &parent_data {
            Some(data) => snapshot::read_file_from_snapshot(data, remote_file_path)?,
            None => None,
        };

        let merge_type = paths::resolve_merge_type(paths_list, remote_file_path);

        let result = merge::merge_file(
            merge_type,
            remote_file_path,
            &local_path,
            &remote_content,
            ancestor_content.as_deref(),
            &state_dir,
            remote_version,
        )?;

        match result {
            MergeResult::Updated => updated.push(remote_file_path.clone()),
            MergeResult::Conflict { .. } => conflict_paths.push(remote_file_path.clone()),
            MergeResult::NewFile => new_files.push(remote_file_path.clone()),
            MergeResult::Unchanged => {}
        }
    }

    // Save remote snapshot as new parent
    std::fs::write(&parent_path, &remote_data)?;

    // Recompute checksums
    let files = collector::collect_files(home_dir, paths_list)?;
    let checksums = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.checksum.clone()))
        .collect();

    let updated_meta = Metadata {
        version: remote_version,
        last_pull: Some(Utc::now()),
        checksums,
        ..meta
    };
    metadata::save_metadata(&updated_meta)?;

    Ok(PullResult {
        version: remote_version,
        updated,
        conflicts: conflict_paths,
        new_files,
        already_up_to_date: false,
    })
}

pub fn cmd_status(
    config: &AppConfig,
    paths_list: &PathsList,
    home_dir: &Path,
) -> Result<StatusReport, SyncError> {
    let meta = metadata::load_metadata()?
        .ok_or(SyncError::NotInitialized)?;

    let backend = create_backend(config)?;
    let remote_version = backend.latest_version(&meta.profile)?;

    let files = collector::collect_files(home_dir, paths_list)?;
    let current_checksums: std::collections::HashMap<String, String> = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.checksum.clone()))
        .collect();

    let mut changed = Vec::new();
    for (path, checksum) in &current_checksums {
        match meta.checksums.get(path) {
            Some(old_checksum) if old_checksum != checksum => {
                changed.push(path.clone());
            }
            None => {
                changed.push(format!("{path} (new)"));
            }
            _ => {}
        }
    }
    for path in meta.checksums.keys() {
        if !current_checksums.contains_key(path) {
            changed.push(format!("{path} (deleted)"));
        }
    }

    changed.sort();

    let state_dir = metadata::state_dir();
    let pending_conflicts = conflicts::list_conflict_versions(&state_dir)
        .unwrap_or_default();

    Ok(StatusReport {
        profile: meta.profile,
        local_version: meta.version,
        remote_version,
        changed_files: changed,
        pending_conflicts,
    })
}

pub fn cmd_sync(
    config: &AppConfig,
    paths_list: &PathsList,
    home_dir: &Path,
    dry_run: bool,
    force: bool,
    quiet: bool,
) -> Result<(), SyncError> {
    if dry_run {
        let meta = metadata::load_metadata()?
            .ok_or(SyncError::NotInitialized)?;
        let files = collector::collect_files(home_dir, paths_list)?;
        let current: std::collections::HashMap<String, String> = files
            .iter()
            .map(|f| (f.relative_path.clone(), f.checksum.clone()))
            .collect();
        let changed: Vec<_> = current
            .keys()
            .filter(|k| meta.checksums.get(*k) != current.get(*k))
            .collect();
        if changed.is_empty() {
            println!("Dry run: no local changes to push");
        } else {
            println!("Dry run: {} file(s) would be pushed", changed.len());
            for path in &changed {
                println!("  M {path}");
            }
        }
        return Ok(());
    }

    let max_retries: u32 = 3;
    for attempt in 0..max_retries {
        match cmd_pull(config, paths_list, home_dir, None, false) {
            Ok(result) => {
                if !quiet && !result.already_up_to_date {
                    print!("{result}");
                }
            }
            Err(SyncError::Backend(BackendError::NoSnapshots(_))) => {}
            Err(e) => return Err(e),
        }

        match cmd_push(config, paths_list, home_dir, force, None) {
            Ok(version) => {
                if !quiet {
                    println!("Synced to version {version}");
                }
                return Ok(());
            }
            Err(SyncError::Backend(BackendError::VersionConflict { local, remote })) => {
                if attempt + 1 < max_retries {
                    log::warn!(
                        "Version conflict (local={local} remote={remote}), retrying {}/{}…",
                        attempt + 1,
                        max_retries
                    );
                    continue;
                }
                return Err(SyncError::Backend(BackendError::VersionConflict { local, remote }));
            }
            Err(e) => return Err(e),
        }
    }

    unreachable!()
}

pub fn cmd_profiles_merge(
    config: &AppConfig,
    from_profile: &str,
    to_profile: &str,
) -> Result<(), SyncError> {
    let backend = create_backend(config)?;
    let (_version, data) = backend.pull(from_profile, None)?;
    backend.push(to_profile, &data, Some(&format!("merged from {from_profile}")))?;
    Ok(())
}

pub fn cmd_log(config: &AppConfig, n: usize) -> Result<Vec<crate::backend::SnapshotMeta>, SyncError> {
    let meta = metadata::load_metadata()?.ok_or(SyncError::NotInitialized)?;
    let backend = create_backend(config)?;
    let mut versions = backend.list_versions(&meta.profile)?;
    versions.sort_by_key(|v| std::cmp::Reverse(v.version));
    versions.truncate(n);
    Ok(versions)
}

pub fn cmd_diff(
    config: &AppConfig,
    paths_list: &PathsList,
    home_dir: &Path,
    remote: bool,
) -> Result<(Vec<String>, Vec<String>, Vec<String>), SyncError> {
    let meta = metadata::load_metadata()?.ok_or(SyncError::NotInitialized)?;

    let snapshot_data = if remote {
        let backend = create_backend(config)?;
        let (_v, data) = backend.pull(&meta.profile, None)?;
        encryption::decrypt(&data, &config.encryption)?
    } else {
        let parent_path = metadata::parent_snapshot_path();
        if !parent_path.exists() {
            return Ok((vec![], vec![], vec![]));
        }
        let data = std::fs::read(&parent_path)?;
        encryption::decrypt(&data, &config.encryption)?
    };

    let snap_files = snapshot::list_snapshot_contents(&snapshot_data)?;
    let snap_set: std::collections::HashSet<&str> = snap_files.iter().map(|s| s.as_str()).collect();

    let local_files = collector::collect_files(home_dir, paths_list)?;
    let local_map: std::collections::HashMap<String, ()> =
        local_files.iter().map(|f| (f.relative_path.clone(), ())).collect();

    let mut modified = vec![];
    let mut deleted = vec![];

    for snap_path in &snap_files {
        let local_path = home_dir.join(snap_path);
        if !local_path.exists() {
            deleted.push(snap_path.clone());
            continue;
        }
        if let Some(snap_content) = snapshot::read_file_from_snapshot(&snapshot_data, snap_path)? {
            let local_content = std::fs::read(&local_path)?;
            if local_content != snap_content {
                modified.push(snap_path.clone());
            }
        }
    }

    let added: Vec<String> = local_files
        .iter()
        .map(|f| f.relative_path.as_str())
        .filter(|p| !snap_set.contains(p))
        .map(String::from)
        .collect();

    Ok((modified, added, deleted))
}

pub fn cmd_restore(
    config: &AppConfig,
    home_dir: &Path,
    version: u64,
    path: Option<&str>,
) -> Result<Vec<String>, SyncError> {
    let meta = metadata::load_metadata()?.ok_or(SyncError::NotInitialized)?;
    let backend = create_backend(config)?;
    let (_v, data) = backend.pull(&meta.profile, Some(version))?;
    let data = encryption::decrypt(&data, &config.encryption)?;
    let snap_files = snapshot::list_snapshot_contents(&data)?;
    let extracted = snapshot::extract_snapshot(&data)?;

    let mut restored = vec![];
    for snap_path in &snap_files {
        if let Some(filter) = path {
            if snap_path != filter {
                continue;
            }
        }
        let src = extracted.path().join(snap_path);
        let dst = home_dir.join(snap_path);
        if src.exists() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
            restored.push(snap_path.clone());
        }
    }
    Ok(restored)
}

pub fn cmd_conflicts_list() -> Result<Vec<(u64, Vec<String>)>, SyncError> {
    let state_dir = metadata::state_dir();
    let versions = conflicts::list_conflict_versions(&state_dir)
        .map_err(|e| SyncError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    let mut result = vec![];
    for v in versions {
        let files = conflicts::list_conflicts_for_version(&state_dir, v)
            .map_err(|e| SyncError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        result.push((v, files));
    }
    Ok(result)
}

pub fn cmd_conflicts_show(home_dir: &Path, path: &str) -> Result<(Vec<u8>, Vec<u8>), SyncError> {
    let state_dir = metadata::state_dir();
    let versions = conflicts::list_conflict_versions(&state_dir)
        .map_err(|e| SyncError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

    let latest_version = versions.last().copied().ok_or_else(|| {
        SyncError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "no conflicts found"))
    })?;

    let stash_path = state_dir.join("conflicts").join(format!("v{latest_version}")).join(path);
    if !stash_path.exists() {
        return Err(SyncError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no conflict stash for '{path}' in version {latest_version}"),
        )));
    }
    let local_path = home_dir.join(path);
    let local = if local_path.exists() { std::fs::read(&local_path)? } else { vec![] };
    let remote = std::fs::read(&stash_path)?;
    Ok((local, remote))
}

pub fn cmd_conflicts_accept_local(path: &str) -> Result<(), SyncError> {
    let state_dir = metadata::state_dir();
    let versions = conflicts::list_conflict_versions(&state_dir)
        .map_err(|e| SyncError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    for v in versions {
        let stash = state_dir.join("conflicts").join(format!("v{v}")).join(path);
        if stash.exists() {
            std::fs::remove_file(&stash)?;
        }
    }
    Ok(())
}

pub fn cmd_conflicts_accept_remote(home_dir: &Path, path: &str) -> Result<(), SyncError> {
    let state_dir = metadata::state_dir();
    let versions = conflicts::list_conflict_versions(&state_dir)
        .map_err(|e| SyncError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

    let latest = versions.last().copied().ok_or_else(|| {
        SyncError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "no conflicts found"))
    })?;

    let stash = state_dir.join("conflicts").join(format!("v{latest}")).join(path);
    if !stash.exists() {
        return Err(SyncError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no conflict stash for '{path}'"),
        )));
    }
    let dst = home_dir.join(path);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&stash, &dst)?;
    // Remove all stashes for this path
    for v in &versions {
        let s = state_dir.join("conflicts").join(format!("v{v}")).join(path);
        if s.exists() { let _ = std::fs::remove_file(&s); }
    }
    Ok(())
}

pub fn cmd_du(
    paths_list: &PathsList,
    home_dir: &Path,
) -> Result<DuReport, SyncError> {
    let files = collector::collect_files(home_dir, paths_list)?;

    let warnings: Vec<String> = collector::check_size_limits(&files)
        .iter()
        .map(|w| w.message.clone())
        .collect();

    let total = collector::total_size(&files);

    let mut file_sizes: Vec<(String, u64)> = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.size))
        .collect();
    file_sizes.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(DuReport {
        files: file_sizes,
        total_bytes: total,
        warnings,
    })
}
