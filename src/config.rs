use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(PathBuf),
    #[error("failed to parse config: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub backend: BackendConfig,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub encryption: EncryptionConfig,
    #[serde(default)]
    pub environments: Vec<EnvironmentRule>,
}

fn default_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BackendConfig {
    #[serde(rename = "local")]
    Local { path: PathBuf },
    #[serde(rename = "s3")]
    S3 {
        bucket: String,
        region: Option<String>,
    },
    #[serde(rename = "sftp")]
    Sftp {
        host: String,
        path: PathBuf,
        key: Option<PathBuf>,
    },
    #[serde(rename = "git")]
    Git { repo: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EncryptionConfig {
    #[serde(default)]
    pub enabled: bool,
    pub identity: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentRule {
    pub name: String,
    #[serde(rename = "match")]
    pub match_rules: MatchRules,
    pub default_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchRules {
    pub hostname: Option<String>,
}

pub fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("opendotsync")
}

pub fn default_config_path() -> PathBuf {
    default_config_dir().join("config.yaml")
}

pub fn load_config(config_path: Option<&Path>) -> Result<AppConfig, ConfigError> {
    let path = config_path
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);

    if !path.exists() {
        return Err(ConfigError::NotFound(path));
    }

    let contents = std::fs::read_to_string(&path)?;
    let config: AppConfig =
        serde_yml::from_str(&contents).map_err(|e| ConfigError::Parse(e.to_string()))?;
    Ok(config)
}

pub fn save_config(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    let yaml = serde_yml::to_string(config).map_err(|e| ConfigError::Parse(e.to_string()))?;
    std::fs::write(path, yaml)?;
    Ok(())
}

pub fn write_default_config(
    path: &Path,
    backend: &BackendConfig,
    profile: &str,
) -> Result<(), ConfigError> {
    let config = AppConfig {
        backend: backend.clone(),
        profile: profile.to_string(),
        encryption: EncryptionConfig::default(),
        environments: Vec::new(),
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let yaml = serde_yml::to_string(&config).map_err(|e| ConfigError::Parse(e.to_string()))?;
    std::fs::write(path, yaml)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let config = AppConfig {
            backend: BackendConfig::Local {
                path: PathBuf::from("/tmp/test-store"),
            },
            profile: "personal".to_string(),
            encryption: EncryptionConfig::default(),
            environments: vec![EnvironmentRule {
                name: "laptop".to_string(),
                match_rules: MatchRules {
                    hostname: Some("dan-mbp*".to_string()),
                },
                default_profile: "personal".to_string(),
            }],
        };

        let yaml = serde_yml::to_string(&config).unwrap();
        let parsed: AppConfig = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(parsed.profile, "personal");
        match &parsed.backend {
            BackendConfig::Local { path } => {
                assert_eq!(path, &PathBuf::from("/tmp/test-store"));
            }
            _ => panic!("expected Local backend"),
        }
        assert_eq!(parsed.environments.len(), 1);
        assert_eq!(parsed.environments[0].name, "laptop");
    }

    #[test]
    fn write_and_load_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yaml");

        let backend = BackendConfig::Local {
            path: PathBuf::from("/tmp/store"),
        };
        write_default_config(&config_path, &backend, "test").unwrap();

        let loaded = load_config(Some(&config_path)).unwrap();
        assert_eq!(loaded.profile, "test");
    }
}
