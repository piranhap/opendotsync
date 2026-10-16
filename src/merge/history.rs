use std::path::Path;

use super::{MergeError, MergeResult};
use crate::collector::compute_checksum_bytes;

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub timestamp: i64,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HistoryFormat {
    Bash,
    Zsh,
    Fish,
}

type HistoryCodec = (
    fn(&[u8]) -> Option<Vec<HistoryEntry>>,
    fn(&[HistoryEntry]) -> Vec<u8>,
);

fn detect_format(relative_path: &str) -> HistoryFormat {
    let filename = Path::new(relative_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    match filename {
        ".zsh_history" => HistoryFormat::Zsh,
        "fish_history" => HistoryFormat::Fish,
        _ => HistoryFormat::Bash,
    }
}

fn contains_within_window(haystack: &[HistoryEntry], needle: &HistoryEntry, window: i64) -> bool {
    haystack
        .iter()
        .any(|e| e.command == needle.command && (e.timestamp - needle.timestamp).abs() <= window)
}

pub fn three_way_merge(
    local: &[HistoryEntry],
    remote: &[HistoryEntry],
    ancestor: &[HistoryEntry],
) -> Vec<HistoryEntry> {
    let mut result: Vec<HistoryEntry> = ancestor.to_vec();

    for entry in local {
        if !contains_within_window(ancestor, entry, 60) {
            result.push(entry.clone());
        }
    }

    for entry in remote {
        if !contains_within_window(ancestor, entry, 60)
            && !contains_within_window(&result, entry, 60)
        {
            result.push(entry.clone());
        }
    }

    result.sort_by_key(|e| e.timestamp);
    result
}

pub fn merge(
    relative_path: &str,
    local_path: &Path,
    remote_content: &[u8],
    ancestor_content: Option<&[u8]>,
    state_dir: &Path,
    remote_version: u64,
) -> Result<MergeResult, MergeError> {
    if !local_path.exists() {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, remote_content)?;
        return Ok(MergeResult::NewFile);
    }

    let local_content = std::fs::read(local_path)?;

    let format = detect_format(relative_path);

    let (parse_fn, serialize_fn): HistoryCodec = match format {
        HistoryFormat::Bash => (super::history_bash::parse, super::history_bash::serialize),
        HistoryFormat::Zsh => (super::history_zsh::parse, super::history_zsh::serialize),
        HistoryFormat::Fish => (super::history_fish::parse, super::history_fish::serialize),
    };

    let local_entries = match parse_fn(&local_content) {
        Some(e) => e,
        None => {
            return super::none::merge(
                relative_path,
                local_path,
                remote_content,
                ancestor_content,
                state_dir,
                remote_version,
            );
        }
    };

    let remote_entries = match parse_fn(remote_content) {
        Some(e) => e,
        None => {
            return super::none::merge(
                relative_path,
                local_path,
                remote_content,
                ancestor_content,
                state_dir,
                remote_version,
            );
        }
    };

    let ancestor_entries = match ancestor_content {
        Some(data) => parse_fn(data).unwrap_or_default(),
        None => Vec::new(),
    };

    let merged = three_way_merge(&local_entries, &remote_entries, &ancestor_entries);
    let merged_bytes = serialize_fn(&merged);

    if compute_checksum_bytes(&local_content) == compute_checksum_bytes(&merged_bytes) {
        return Ok(MergeResult::Unchanged);
    }

    std::fs::write(local_path, &merged_bytes)?;
    Ok(MergeResult::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_way_no_overlap() {
        let ancestor = vec![HistoryEntry {
            timestamp: 100,
            command: "ls".into(),
        }];
        let local = vec![
            HistoryEntry {
                timestamp: 100,
                command: "ls".into(),
            },
            HistoryEntry {
                timestamp: 200,
                command: "cd /tmp".into(),
            },
        ];
        let remote = vec![
            HistoryEntry {
                timestamp: 100,
                command: "ls".into(),
            },
            HistoryEntry {
                timestamp: 300,
                command: "git status".into(),
            },
        ];

        let merged = three_way_merge(&local, &remote, &ancestor);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].command, "ls");
        assert_eq!(merged[1].command, "cd /tmp");
        assert_eq!(merged[2].command, "git status");
    }

    #[test]
    fn three_way_dedup_within_60s_window() {
        let ancestor = vec![];
        let local = vec![HistoryEntry {
            timestamp: 100,
            command: "echo hello".into(),
        }];
        let remote = vec![HistoryEntry {
            timestamp: 130,
            command: "echo hello".into(),
        }];

        let merged = three_way_merge(&local, &remote, &ancestor);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn three_way_no_ancestor_union() {
        let ancestor = vec![];
        let local = vec![HistoryEntry {
            timestamp: 100,
            command: "ls".into(),
        }];
        let remote = vec![HistoryEntry {
            timestamp: 200,
            command: "pwd".into(),
        }];

        let merged = three_way_merge(&local, &remote, &ancestor);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|e| e.command == "ls"));
        assert!(merged.iter().any(|e| e.command == "pwd"));
    }

    #[test]
    fn merged_entries_sorted_by_timestamp() {
        let ancestor = vec![];
        let local = vec![HistoryEntry {
            timestamp: 300,
            command: "last".into(),
        }];
        let remote = vec![HistoryEntry {
            timestamp: 100,
            command: "first".into(),
        }];

        let merged = three_way_merge(&local, &remote, &ancestor);
        assert_eq!(merged[0].command, "first");
        assert_eq!(merged[1].command, "last");
    }

    #[test]
    fn detect_format_zsh() {
        assert_eq!(detect_format(".zsh_history"), HistoryFormat::Zsh);
    }

    #[test]
    fn detect_format_bash() {
        assert_eq!(detect_format(".bash_history"), HistoryFormat::Bash);
        assert_eq!(detect_format(".python_history"), HistoryFormat::Bash);
    }

    #[test]
    fn detect_format_fish() {
        assert_eq!(detect_format("fish_history"), HistoryFormat::Fish);
    }
}
