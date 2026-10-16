use std::path::Path;

use super::{MergeError, MergeResult};
use crate::collector::compute_checksum_bytes;
use crate::conflicts;

pub fn merge(
    relative_path: &str,
    local_path: &Path,
    remote_content: &[u8],
    ancestor_content: Option<&[u8]>,
    state_dir: &Path,
    remote_version: u64,
) -> Result<MergeResult, MergeError> {
    // If local file doesn't exist, this is a new file from remote
    if !local_path.exists() {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, remote_content)?;
        return Ok(MergeResult::NewFile);
    }

    let local_content = std::fs::read(local_path)?;
    let local_hash = compute_checksum_bytes(&local_content);
    let remote_hash = compute_checksum_bytes(remote_content);

    // Both identical — no change needed
    if local_hash == remote_hash {
        return Ok(MergeResult::Unchanged);
    }

    match ancestor_content {
        Some(ancestor) => {
            let parent_hash = compute_checksum_bytes(ancestor);

            if local_hash == parent_hash {
                // Only remote changed — accept remote
                std::fs::write(local_path, remote_content)?;
                Ok(MergeResult::Updated)
            } else if remote_hash == parent_hash {
                // Only local changed — keep local
                Ok(MergeResult::Unchanged)
            } else {
                // Both changed — conflict
                let stash_path = conflicts::stash_conflict(
                    state_dir,
                    remote_version,
                    relative_path,
                    remote_content,
                )?;
                log::warn!(
                    "Conflict in {relative_path} — remote stashed in {}",
                    stash_path.display()
                );
                Ok(MergeResult::Conflict { stash_path })
            }
        }
        None => {
            // No ancestor — keep local, stash remote (Section 10.2)
            let stash_path = conflicts::stash_conflict(
                state_dir,
                remote_version,
                relative_path,
                remote_content,
            )?;
            log::warn!(
                "No ancestor for {relative_path} — remote stashed in {}",
                stash_path.display()
            );
            Ok(MergeResult::Conflict { stash_path })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        (home, state)
    }

    #[test]
    fn both_identical() {
        let (home, state) = setup();
        let local_path = home.path().join(".bashrc");
        std::fs::write(&local_path, "same content").unwrap();

        let result = merge(
            ".bashrc",
            &local_path,
            b"same content",
            Some(b"same content"),
            state.path(),
            1,
        )
        .unwrap();

        assert!(matches!(result, MergeResult::Unchanged));
    }

    #[test]
    fn only_remote_changed() {
        let (home, state) = setup();
        let local_path = home.path().join(".bashrc");
        let ancestor = b"original content";
        std::fs::write(&local_path, ancestor).unwrap();

        let result = merge(
            ".bashrc",
            &local_path,
            b"updated by remote",
            Some(ancestor),
            state.path(),
            2,
        )
        .unwrap();

        assert!(matches!(result, MergeResult::Updated));
        let content = std::fs::read_to_string(&local_path).unwrap();
        assert_eq!(content, "updated by remote");
    }

    #[test]
    fn only_local_changed() {
        let (home, state) = setup();
        let local_path = home.path().join(".bashrc");
        let ancestor = b"original content";
        std::fs::write(&local_path, "modified locally").unwrap();

        let result = merge(
            ".bashrc",
            &local_path,
            ancestor,
            Some(ancestor),
            state.path(),
            3,
        )
        .unwrap();

        assert!(matches!(result, MergeResult::Unchanged));
        let content = std::fs::read_to_string(&local_path).unwrap();
        assert_eq!(content, "modified locally");
    }

    #[test]
    fn both_changed_conflict() {
        let (home, state) = setup();
        let local_path = home.path().join(".bashrc");
        std::fs::write(&local_path, "local changes").unwrap();

        let result = merge(
            ".bashrc",
            &local_path,
            b"remote changes",
            Some(b"original"),
            state.path(),
            4,
        )
        .unwrap();

        match result {
            MergeResult::Conflict { stash_path } => {
                assert!(stash_path.exists());
                let stashed = std::fs::read_to_string(&stash_path).unwrap();
                assert_eq!(stashed, "remote changes");
            }
            _ => panic!("expected Conflict"),
        }

        // Local file should be unchanged
        let content = std::fs::read_to_string(&local_path).unwrap();
        assert_eq!(content, "local changes");
    }

    #[test]
    fn no_ancestor_stashes_remote() {
        let (home, state) = setup();
        let local_path = home.path().join(".bashrc");
        std::fs::write(&local_path, "local version").unwrap();

        let result = merge(
            ".bashrc",
            &local_path,
            b"remote version",
            None,
            state.path(),
            5,
        )
        .unwrap();

        assert!(matches!(result, MergeResult::Conflict { .. }));
        let content = std::fs::read_to_string(&local_path).unwrap();
        assert_eq!(content, "local version");
    }

    #[test]
    fn new_file_from_remote() {
        let (home, state) = setup();
        let local_path = home.path().join(".new_config");

        let result = merge(
            ".new_config",
            &local_path,
            b"new file content",
            None,
            state.path(),
            6,
        )
        .unwrap();

        assert!(matches!(result, MergeResult::NewFile));
        assert!(local_path.exists());
        let content = std::fs::read_to_string(&local_path).unwrap();
        assert_eq!(content, "new file content");
    }
}
