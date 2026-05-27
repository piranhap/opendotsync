use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glob::Pattern;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathsError {
    #[error("paths file not found: {0}")]
    NotFound(PathBuf),
    #[error("failed to parse paths: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid glob pattern: {0}")]
    InvalidPattern(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeType {
    None,
    History,
    Union,
}

impl Default for MergeType {
    fn default() -> Self {
        MergeType::None
    }
}

#[derive(Debug, Clone)]
pub struct PathEntry {
    pub path: String,
    pub merge_type: MergeType,
}

#[derive(Debug, Clone)]
pub struct PathsList {
    pub include: Vec<PathEntry>,
    pub exclude: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawPathEntry {
    Plain(String),
    WithType(HashMap<String, MergeType>),
}

#[derive(Deserialize)]
struct RawPathsList {
    #[serde(default)]
    include: Vec<RawPathEntry>,
    #[serde(default)]
    exclude: Vec<String>,
}

impl From<RawPathsList> for PathsList {
    fn from(raw: RawPathsList) -> Self {
        let include = raw
            .include
            .into_iter()
            .map(|entry| match entry {
                RawPathEntry::Plain(path) => PathEntry {
                    path,
                    merge_type: MergeType::None,
                },
                RawPathEntry::WithType(map) => {
                    let (path, merge_type) = map.into_iter().next().unwrap_or_else(|| {
                        ("".to_string(), MergeType::None)
                    });
                    PathEntry { path, merge_type }
                }
            })
            .filter(|e| !e.path.is_empty())
            .collect();

        PathsList {
            include,
            exclude: raw.exclude,
        }
    }
}

pub fn default_paths_path() -> PathBuf {
    crate::config::default_config_dir().join("paths.yaml")
}

pub fn load_paths(path: Option<&Path>) -> Result<PathsList, PathsError> {
    let path = path
        .map(PathBuf::from)
        .unwrap_or_else(default_paths_path);

    if !path.exists() {
        return Err(PathsError::NotFound(path));
    }

    let contents = std::fs::read_to_string(&path)?;
    let raw: RawPathsList =
        serde_yml::from_str(&contents).map_err(|e| PathsError::Parse(e.to_string()))?;
    Ok(PathsList::from(raw))
}

pub fn write_default_paths(path: &Path) -> Result<(), PathsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let default_yaml = r#"include:
  - ".bashrc"
  - ".bash_profile"
  - ".zshrc"
  - ".vimrc"
  - ".gitconfig"
  - ".ssh/config"
  - ".tmux.conf"
  - "bin/"
  - ".config/nvim/"
  - ".bash_history":    history
  - ".zsh_history":     history
  - ".python_history":  history

exclude:
  - ".ssh/id_*"
  - ".ssh/*.pem"
  - ".ssh/known_hosts"
  - ".gnupg/private-keys-v1.d/"
  - ".aws/credentials"
  - ".env"
  - "*.secret"
  - ".netrc"
  - ".kube/config"
  - "*.swp"
  - "*.swo"
  - ".DS_Store"
"#;

    std::fs::write(path, default_yaml)?;
    Ok(())
}

pub fn save_paths(path: &Path, paths_list: &PathsList) -> Result<(), PathsError> {
    use serde_yml::{Mapping, Value};

    let include_values: Vec<Value> = paths_list
        .include
        .iter()
        .map(|entry| {
            if entry.merge_type == MergeType::None {
                Value::String(entry.path.clone())
            } else {
                let mt = match entry.merge_type {
                    MergeType::History => "history",
                    MergeType::Union => "union",
                    MergeType::None => "none",
                };
                let mut map = Mapping::new();
                map.insert(Value::String(entry.path.clone()), Value::String(mt.to_string()));
                Value::Mapping(map)
            }
        })
        .collect();

    let mut root = Mapping::new();
    root.insert(Value::String("include".into()), Value::Sequence(include_values));

    if !paths_list.exclude.is_empty() {
        let excl: Vec<Value> =
            paths_list.exclude.iter().map(|s| Value::String(s.clone())).collect();
        root.insert(Value::String("exclude".into()), Value::Sequence(excl));
    }

    let yaml = serde_yml::to_string(&Value::Mapping(root))
        .map_err(|e| PathsError::Parse(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, yaml)?;
    Ok(())
}

pub fn resolve_merge_type(paths_list: &PathsList, file_path: &str) -> MergeType {
    // 1. Check for exact match
    for entry in &paths_list.include {
        let entry_path = entry.path.trim_end_matches('/');
        if file_path == entry_path || file_path == entry.path {
            return entry.merge_type;
        }
    }

    // 2. Walk up directory tree — check if any parent directory has an annotation
    let path = Path::new(file_path);
    for ancestor in path.ancestors().skip(1) {
        let ancestor_str = ancestor.to_string_lossy();
        if ancestor_str.is_empty() {
            break;
        }
        for entry in &paths_list.include {
            let entry_path = entry.path.trim_end_matches('/');
            if ancestor_str == entry_path {
                return entry.merge_type;
            }
        }
    }

    // 3. Default
    MergeType::None
}

pub fn is_excluded(paths_list: &PathsList, file_path: &str) -> bool {
    for pattern_str in &paths_list.exclude {
        if pattern_str.contains('/') {
            if let Ok(pat) = Pattern::new(pattern_str) {
                if pat.matches(file_path) {
                    return true;
                }
            }
        } else {
            let filename = Path::new(file_path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("");
            if let Ok(pat) = Pattern::new(pattern_str) {
                if pat.matches(filename) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn is_included(paths_list: &PathsList, file_path: &str) -> bool {
    for entry in &paths_list.include {
        // Directory include: "bin/" matches anything under bin/
        if entry.path.ends_with('/') {
            let dir_prefix = &entry.path;
            if file_path.starts_with(dir_prefix.as_str()) {
                return true;
            }
            let dir_no_slash = entry.path.trim_end_matches('/');
            if file_path.starts_with(dir_no_slash)
                && file_path.as_bytes().get(dir_no_slash.len()) == Some(&b'/')
            {
                return true;
            }
        }

        // Exact match
        if file_path == entry.path {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mixed_paths_yaml() {
        let yaml = r#"
include:
  - ".bashrc"
  - ".bash_history": history
  - ".config/claude/": union
exclude:
  - "*.swp"
  - ".ssh/id_*"
"#;
        let raw: RawPathsList = serde_yml::from_str(yaml).unwrap();
        let paths = PathsList::from(raw);

        assert_eq!(paths.include.len(), 3);
        assert_eq!(paths.include[0].path, ".bashrc");
        assert_eq!(paths.include[0].merge_type, MergeType::None);
        assert_eq!(paths.include[1].path, ".bash_history");
        assert_eq!(paths.include[1].merge_type, MergeType::History);
        assert_eq!(paths.include[2].path, ".config/claude/");
        assert_eq!(paths.include[2].merge_type, MergeType::Union);
        assert_eq!(paths.exclude.len(), 2);
    }

    #[test]
    fn resolve_merge_type_exact_match() {
        let paths = PathsList {
            include: vec![
                PathEntry {
                    path: ".bashrc".to_string(),
                    merge_type: MergeType::None,
                },
                PathEntry {
                    path: ".bash_history".to_string(),
                    merge_type: MergeType::History,
                },
            ],
            exclude: vec![],
        };

        assert_eq!(resolve_merge_type(&paths, ".bashrc"), MergeType::None);
        assert_eq!(
            resolve_merge_type(&paths, ".bash_history"),
            MergeType::History
        );
    }

    #[test]
    fn resolve_merge_type_directory_inheritance() {
        let paths = PathsList {
            include: vec![PathEntry {
                path: ".config/claude/".to_string(),
                merge_type: MergeType::Union,
            }],
            exclude: vec![],
        };

        assert_eq!(
            resolve_merge_type(&paths, ".config/claude/settings.json"),
            MergeType::Union
        );
    }

    #[test]
    fn exclude_filename_pattern() {
        let paths = PathsList {
            include: vec![],
            exclude: vec!["*.swp".to_string()],
        };

        assert!(is_excluded(&paths, ".vimrc.swp"));
        assert!(is_excluded(&paths, "src/foo.swp"));
        assert!(!is_excluded(&paths, ".bashrc"));
    }

    #[test]
    fn exclude_path_pattern() {
        let paths = PathsList {
            include: vec![],
            exclude: vec![".ssh/id_*".to_string()],
        };

        assert!(is_excluded(&paths, ".ssh/id_rsa"));
        assert!(is_excluded(&paths, ".ssh/id_ed25519"));
        assert!(!is_excluded(&paths, ".ssh/config"));
    }

    #[test]
    fn include_directory() {
        let paths = PathsList {
            include: vec![PathEntry {
                path: "bin/".to_string(),
                merge_type: MergeType::None,
            }],
            exclude: vec![],
        };

        assert!(is_included(&paths, "bin/myscript.sh"));
        assert!(is_included(&paths, "bin/tools/helper.py"));
        assert!(!is_included(&paths, ".bashrc"));
    }

    #[test]
    fn write_and_load_paths() {
        let dir = tempfile::tempdir().unwrap();
        let paths_path = dir.path().join("paths.yaml");

        write_default_paths(&paths_path).unwrap();

        let loaded = load_paths(Some(&paths_path)).unwrap();
        assert!(!loaded.include.is_empty());
        assert!(!loaded.exclude.is_empty());

        let history_entry = loaded
            .include
            .iter()
            .find(|e| e.path == ".bash_history")
            .expect("should have .bash_history");
        assert_eq!(history_entry.merge_type, MergeType::History);
    }
}
