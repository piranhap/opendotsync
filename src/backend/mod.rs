mod git;
pub mod github;
pub mod local;
pub mod rclone;
mod s3;
mod sftp;

use std::io::Write;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Enable or disable echoing subprocess (git/rclone) output to the terminal.
/// Set once from the CLI `--verbose` flag; suppressed by default.
pub fn set_verbose(verbose: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Run `cmd` capturing stdout/stderr instead of inheriting the terminal, so
/// subprocess chatter never leaks into opendotsync's own output. When verbose
/// is enabled the captured output is echoed. The caller decides whether a
/// non-zero exit is an error.
pub(crate) fn run_captured(cmd: &mut Command) -> Result<Output, BackendError> {
    let out = cmd.output()?;
    if verbose() {
        let _ = std::io::stdout().write_all(&out.stdout);
        let _ = std::io::stderr().write_all(&out.stderr);
    }
    Ok(out)
}

/// Like [`run_captured`], but a non-zero exit becomes a `BackendError` whose
/// message includes the captured stderr.
pub(crate) fn run_checked(cmd: &mut Command, context: &str) -> Result<Output, BackendError> {
    let out = run_captured(cmd)?;
    if out.status.success() {
        return Ok(out);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    let msg = if stderr.is_empty() {
        context.to_string()
    } else {
        format!("{context}: {stderr}")
    };
    Err(BackendError::Io(std::io::Error::other(msg)))
}

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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn run_captured_collects_stdout_and_stderr() {
        let out = run_captured(Command::new("sh").args(["-c", "echo out; echo err >&2"])).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
    }

    #[test]
    fn run_checked_includes_stderr_on_failure() {
        let err = run_checked(
            Command::new("sh").args(["-c", "echo boom >&2; exit 3"]),
            "fake cmd failed",
        )
        .unwrap_err();
        assert!(err.to_string().contains("fake cmd failed"), "{err}");
        assert!(err.to_string().contains("boom"), "{err}");
    }

    #[test]
    fn run_checked_without_stderr_keeps_context_only() {
        let err =
            run_checked(Command::new("sh").args(["-c", "exit 3"]), "fake cmd failed").unwrap_err();
        assert_eq!(err.to_string(), "IO error: fake cmd failed");
    }
}

pub trait StorageBackend {
    fn push(&self, profile: &str, data: &[u8], message: Option<&str>) -> Result<u64, BackendError>;

    fn pull(&self, profile: &str, version: Option<u64>) -> Result<(u64, Vec<u8>), BackendError>;

    fn list_versions(&self, profile: &str) -> Result<Vec<SnapshotMeta>, BackendError>;

    fn prune(&self, profile: &str, keep: usize) -> Result<(), BackendError>;

    fn latest_version(&self, profile: &str) -> Result<Option<u64>, BackendError>;
}
