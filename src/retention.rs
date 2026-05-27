use crate::backend::{BackendError, StorageBackend};

pub const DEFAULT_KEEP: usize = 10;

/// Prune old snapshots, keeping only the `keep` most recent.
/// Returns the number of versions deleted.
pub fn prune(
    backend: &dyn StorageBackend,
    profile: &str,
    keep: usize,
) -> Result<usize, BackendError> {
    let versions = backend.list_versions(profile)?;
    if versions.len() <= keep {
        return Ok(0);
    }
    let to_delete = versions.len() - keep;
    backend.prune(profile, keep)?;
    Ok(to_delete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::local::LocalBackend;

    #[test]
    fn prune_removes_old_versions() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_path_buf());
        for i in 0..5u8 {
            backend.push("test", &[i], None).unwrap();
        }
        let deleted = prune(&backend, "test", 3).unwrap();
        assert_eq!(deleted, 2);
        let remaining = backend.list_versions("test").unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(remaining[0].version, 3);
    }

    #[test]
    fn prune_noop_when_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_path_buf());
        backend.push("test", b"a", None).unwrap();
        let deleted = prune(&backend, "test", 10).unwrap();
        assert_eq!(deleted, 0);
    }
}
