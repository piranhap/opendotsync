use std::path::Path;

use thiserror::Error;

use crate::config::{AppConfig, BackendConfig, ConfigError};

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("invalid profile name '{0}' — use alphanumeric, hyphens, or underscores only")]
    InvalidName(String),
}

pub fn current_profile(config: &AppConfig) -> &str {
    &config.profile
}

/// Match current hostname against environment rules; return first matching profile name.
pub fn detect_profile(config: &AppConfig) -> Option<String> {
    if config.environments.is_empty() {
        return None;
    }
    let hostname = hostname::get().ok()?.into_string().ok()?;
    for env in &config.environments {
        if let Some(pattern) = &env.match_rules.hostname {
            if glob_match(pattern, &hostname) {
                return Some(env.default_profile.clone());
            }
        }
    }
    None
}

fn glob_match(pattern: &str, s: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        s.starts_with(prefix)
    } else if let Some(suffix) = pattern.strip_prefix('*') {
        s.ends_with(suffix)
    } else {
        pattern == s
    }
}

fn validate_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ProfileError::InvalidName(name.to_string()));
    }
    Ok(())
}

pub fn list_profiles(config: &AppConfig) -> Result<Vec<String>, ProfileError> {
    match &config.backend {
        BackendConfig::Local { path } => {
            if !path.exists() {
                return Ok(vec![]);
            }
            let mut profiles: Vec<String> = std::fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            profiles.sort();
            Ok(profiles)
        }
        _ => Ok(vec![config.profile.clone()]),
    }
}

pub fn create_profile(name: &str) -> Result<(), ProfileError> {
    validate_name(name)?;
    // Profile directories are created on first push; nothing else to do here.
    Ok(())
}

pub fn switch_profile(
    config_path: &Path,
    name: &str,
    current: &AppConfig,
) -> Result<(), ProfileError> {
    validate_name(name)?;
    let mut updated = current.clone();
    updated.profile = name.to_string();
    crate::config::save_config(config_path, &updated)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EncryptionConfig, EnvironmentRule, MatchRules};

    fn local_config(path: &std::path::Path) -> AppConfig {
        AppConfig {
            backend: BackendConfig::Local {
                path: path.to_path_buf(),
            },
            profile: "default".to_string(),
            encryption: EncryptionConfig::default(),
            environments: vec![],
        }
    }

    #[test]
    fn list_empty_backend() {
        let dir = tempfile::tempdir().unwrap();
        let config = local_config(&dir.path().join("store"));
        assert_eq!(list_profiles(&config).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn list_profiles_finds_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(store.join("default")).unwrap();
        std::fs::create_dir_all(store.join("work")).unwrap();
        let config = local_config(&store);
        assert_eq!(list_profiles(&config).unwrap(), vec!["default", "work"]);
    }

    #[test]
    fn switch_updates_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");
        let config = local_config(&dir.path().join("store"));
        crate::config::save_config(&config_path, &config).unwrap();

        switch_profile(&config_path, "work", &config).unwrap();

        let loaded = crate::config::load_config(Some(&config_path)).unwrap();
        assert_eq!(loaded.profile, "work");
    }

    #[test]
    fn invalid_name_rejected() {
        assert!(matches!(
            create_profile("bad name!"),
            Err(ProfileError::InvalidName(_))
        ));
    }

    #[test]
    fn detect_profile_no_panic() {
        let config = AppConfig {
            backend: BackendConfig::Local {
                path: "/tmp".into(),
            },
            profile: "default".to_string(),
            encryption: EncryptionConfig::default(),
            environments: vec![EnvironmentRule {
                name: "work".to_string(),
                match_rules: MatchRules {
                    hostname: Some("NOMATCH-*".to_string()),
                },
                default_profile: "work".to_string(),
            }],
        };
        let _ = detect_profile(&config);
    }
}
