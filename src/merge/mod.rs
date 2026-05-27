pub mod none;
mod history;
mod history_bash;
mod history_zsh;
mod history_fish;
mod union;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use crate::paths::MergeType;

#[derive(Debug, Error)]
pub enum MergeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("conflict error: {0}")]
    Conflict(#[from] crate::conflicts::ConflictError),
    #[error("merge strategy not yet implemented: {0:?}")]
    NotImplemented(MergeType),
}

#[derive(Debug)]
pub enum MergeResult {
    Unchanged,
    Updated,
    Conflict { stash_path: PathBuf },
    NewFile,
}

pub fn merge_file(
    merge_type: MergeType,
    relative_path: &str,
    local_path: &Path,
    remote_content: &[u8],
    ancestor_content: Option<&[u8]>,
    state_dir: &Path,
    remote_version: u64,
) -> Result<MergeResult, MergeError> {
    match merge_type {
        MergeType::None => none::merge(
            relative_path,
            local_path,
            remote_content,
            ancestor_content,
            state_dir,
            remote_version,
        ),
        MergeType::History => {
            log::warn!(
                "History merge not yet implemented for {relative_path}, falling back to last-write-wins"
            );
            none::merge(
                relative_path,
                local_path,
                remote_content,
                ancestor_content,
                state_dir,
                remote_version,
            )
        }
        MergeType::Union => {
            log::warn!(
                "Union merge not yet implemented for {relative_path}, falling back to last-write-wins"
            );
            none::merge(
                relative_path,
                local_path,
                remote_content,
                ancestor_content,
                state_dir,
                remote_version,
            )
        }
    }
}
