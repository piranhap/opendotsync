use std::collections::HashSet;
use std::path::Path;

use super::{MergeError, MergeResult};

pub fn merge(
    relative_path: &str,
    local_path: &Path,
    remote_content: &[u8],
    ancestor_content: Option<&[u8]>,
    state_dir: &Path,
    remote_version: u64,
) -> Result<MergeResult, MergeError> {
    let remote_text = match std::str::from_utf8(remote_content) {
        Ok(s) => s,
        Err(_) => {
            return super::none::merge(
                relative_path,
                local_path,
                remote_content,
                ancestor_content,
                state_dir,
                remote_version,
            )
        }
    };

    if !local_path.exists() {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, remote_content)?;
        return Ok(MergeResult::NewFile);
    }

    let local_bytes = std::fs::read(local_path)?;
    let local_text = match std::str::from_utf8(&local_bytes) {
        Ok(s) => s,
        Err(_) => {
            return super::none::merge(
                relative_path,
                local_path,
                remote_content,
                ancestor_content,
                state_dir,
                remote_version,
            )
        }
    };

    if local_bytes == remote_content {
        return Ok(MergeResult::Unchanged);
    }

    let local_lines: Vec<&str> = local_text.lines().collect();
    let remote_lines: Vec<&str> = remote_text.lines().collect();
    let merged = union_lines(&local_lines, &remote_lines);

    let trailing = local_text.ends_with('\n') || remote_text.ends_with('\n');
    let mut result = merged.join("\n");
    if trailing && !result.is_empty() {
        result.push('\n');
    }

    if result.as_bytes() == local_bytes.as_slice() {
        return Ok(MergeResult::Unchanged);
    }

    let parent = local_path.parent().unwrap_or(Path::new("/tmp"));
    let tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::fs::write(tmp.path(), result.as_bytes())?;
    tmp.persist(local_path).map_err(|e| e.error)?;

    Ok(MergeResult::Updated)
}

fn union_lines<'a>(local: &[&'a str], remote: &[&'a str]) -> Vec<&'a str> {
    let local_set: HashSet<&str> = local.iter().copied().collect();
    let mut result: Vec<&str> = local.to_vec();
    for &line in remote {
        if !local_set.contains(line) {
            result.push(line);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_remote_only_lines() {
        assert_eq!(union_lines(&["a", "b"], &["b", "c"]), vec!["a", "b", "c"]);
    }

    #[test]
    fn no_duplicates_when_identical() {
        let lines = vec!["x", "y", "z"];
        assert_eq!(union_lines(&lines, &lines), lines);
    }

    #[test]
    fn empty_remote() {
        assert_eq!(union_lines(&["a", "b"], &[]), vec!["a", "b"]);
    }

    #[test]
    fn empty_local() {
        assert_eq!(union_lines(&[], &["a", "b"]), vec!["a", "b"]);
    }

    #[test]
    fn merge_writes_union_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let local_path = dir.path().join("aliases");
        std::fs::write(&local_path, "alias ll='ls -la'\nalias gs='git status'\n").unwrap();
        let remote = b"alias gs='git status'\nalias gp='git push'\n";
        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        let result = merge(".aliases", &local_path, remote, None, &state_dir, 1).unwrap();
        assert!(matches!(result, MergeResult::Updated));

        let content = std::fs::read_to_string(&local_path).unwrap();
        assert!(content.contains("alias ll='ls -la'"));
        assert!(content.contains("alias gs='git status'"));
        assert!(content.contains("alias gp='git push'"));
        assert_eq!(content.matches("alias ll").count(), 1);
    }
}
